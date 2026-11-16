use crate::remote_protocol::{self};
use crate::{Result, db_error, fail, now, require, valid_id};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use rand::{RngCore, rngs::OsRng};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{FromRow, PgPool, Postgres, Transaction};
#[derive(Clone)]
pub struct RemoteRobots {
    pub pool: PgPool,
}
#[derive(Debug, Clone, FromRow)]
struct Device {
    id: String,
    name: String,
    created: f64,
    token_hash: Option<String>,
    agent_session: Option<String>,
    seen: f64,
    lease: Option<String>,
    lease_until: f64,
    telemetry: Option<Value>,
    map: Option<Value>,
    image: Option<String>,
    stop_epoch: i64,
}
#[derive(Debug, Clone, FromRow)]
struct Command {
    id: String,
    kind: String,
    state: String,
    created: f64,
    expires: f64,
    payload: Value,
    error: Option<String>,
    updated: f64,
    accepted_at: Option<f64>,
}
fn secret(size: usize) -> Result<String> {
    let mut bytes = vec![0; size];
    OsRng
        .try_fill_bytes(&mut bytes)
        .map_err(|_| fail("Secure randomness unavailable", 503))?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}
fn hash(value: &str) -> String {
    format!("{:x}", Sha256::digest(value.as_bytes()))
}
fn target<'a>(body: &'a Value, key: &str) -> Result<&'a str> {
    body.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| fail(format!("Invalid {key}"), 400))
}
async fn owned(tx: &mut Transaction<'_, Postgres>, owner: &str, id: &str) -> Result<Device> {
    sqlx::query_as::<_, Device>(
        "SELECT * FROM ai_store.remote_robots WHERE id=$1 AND owner=$2 AND NOT revoked FOR UPDATE",
    )
    .bind(id)
    .bind(owner)
    .fetch_optional(&mut **tx)
    .await
    .map_err(db_error)?
    .ok_or_else(|| fail("기기를 찾을 수 없어요.", 404))
}
async fn stop_tx(tx: &mut Transaction<'_, Postgres>, id: &str, at: f64, error: &str) -> Result<()> {
    sqlx::query("UPDATE ai_store.robot_commands SET state='cancelled',error=$1,updated=$2 WHERE device=$3 AND state IN('queued','dispatched','running')").bind(error).bind(at).bind(id).execute(&mut **tx).await.map_err(db_error)?;
    sqlx::query("UPDATE ai_store.remote_robots SET stop_epoch=stop_epoch+1,lease_until=0,lease=NULL WHERE id=$1").bind(id).execute(&mut **tx).await.map_err(db_error)?;
    Ok(())
}
fn command_public(command: Command) -> Value {
    json!({"id":command.id,"kind":command.kind,"state":command.state,"created":command.created,"expires":command.expires,"payload":command.payload,"error":command.error,"updated":command.updated,"accepted_at":command.accepted_at})
}
impl RemoteRobots {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
    async fn latest(&self, id: &str) -> Result<Value> {
        Ok(sqlx::query_as::<_,Command>("SELECT id,kind,state,created,expires,payload,error,updated,accepted_at FROM ai_store.robot_commands WHERE device=$1 ORDER BY created DESC,id DESC LIMIT 1").bind(id).fetch_optional(&self.pool).await.map_err(db_error)?.map(command_public).unwrap_or(Value::Null))
    }
    async fn public(&self, row: Device) -> Result<Value> {
        Ok(
            json!({"id":row.id,"name":row.name,"created":row.created,"online":now()-row.seen<4.,"last_seen":row.seen,"paired":row.token_hash.is_some(),"control_until":row.lease_until,"telemetry":row.telemetry,"map":row.map,"image":row.image,"command":self.latest(&row.id).await?,"stop_epoch":row.stop_epoch}),
        )
    }
    pub async fn list(&self, owner: &str) -> Result<Value> {
        let rows=sqlx::query_as::<_,Device>("SELECT * FROM ai_store.remote_robots WHERE owner=$1 AND NOT revoked ORDER BY created DESC LIMIT 20").bind(owner).fetch_all(&self.pool).await.map_err(db_error)?;
        Ok(Value::Array(rows.into_iter().map(|row|json!({"id":row.id,"name":row.name,"online":now()-row.seen<4.,"paired":row.token_hash.is_some(),"last_seen":row.seen})).collect()))
    }
    pub async fn create(&self, owner: &str, name: &str) -> Result<Value> {
        require(valid_id(owner), "Authentication required")?;
        require(
            (1..=60).contains(&name.trim().chars().count()),
            "기기 이름은 1–60자로 입력하세요.",
        )?;
        let mut tx = self.pool.begin().await.map_err(db_error)?;
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended('robotics:'||$1,0))")
            .bind(owner)
            .execute(&mut *tx)
            .await
            .map_err(db_error)?;
        let count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM ai_store.remote_robots WHERE owner=$1 AND NOT revoked",
        )
        .bind(owner)
        .fetch_one(&mut *tx)
        .await
        .map_err(db_error)?;
        if count >= 10 {
            return Err(fail("기기는 최대 10개 등록할 수 있어요.", 429));
        }
        let id = uuid::Uuid::new_v4().to_string();
        let code = secret(24)?;
        let at = now();
        sqlx::query("INSERT INTO ai_store.remote_robots(id,owner,name,created,pair_hash,pair_until) VALUES($1,$2,$3,$4,$5,$6)").bind(&id).bind(owner).bind(name.trim()).bind(at).bind(hash(&code)).bind(at+600.).execute(&mut *tx).await.map_err(db_error)?;
        let row = owned(&mut tx, owner, &id).await?;
        tx.commit().await.map_err(db_error)?;
        Ok(json!({"device":self.public(row).await?,"pair_code":code,"expires_at":at+600.}))
    }
    pub async fn re_pair(&self, owner: &str, id: &str) -> Result<Value> {
        let mut tx = self.pool.begin().await.map_err(db_error)?;
        owned(&mut tx, owner, id).await?;
        let at = now();
        stop_tx(&mut tx, id, at, "Stopped by operator").await?;
        let code = secret(24)?;
        sqlx::query("UPDATE ai_store.remote_robots SET token_hash=NULL,pair_hash=$1,pair_until=$2,agent_session=NULL,seen=0 WHERE id=$3 AND owner=$4").bind(hash(&code)).bind(at+600.).bind(id).bind(owner).execute(&mut *tx).await.map_err(db_error)?;
        tx.commit().await.map_err(db_error)?;
        Ok(json!({"pair_code":code,"expires_at":at+600.}))
    }
    pub async fn state(&self, owner: &str, id: &str) -> Result<Value> {
        let mut tx = self.pool.begin().await.map_err(db_error)?;
        let row = owned(&mut tx, owner, id).await?;
        let at = now();
        if at - row.seen > 4. || row.lease_until < at {
            sqlx::query("UPDATE ai_store.robot_commands SET state='cancelled',error='Device or operator disconnected',updated=$1 WHERE device=$2 AND state IN('queued','dispatched','running')").bind(at).bind(id).execute(&mut *tx).await.map_err(db_error)?;
        }
        tx.commit().await.map_err(db_error)?;
        self.public(row).await
    }
    pub async fn control(&self, owner: &str, id: &str, body: &Value) -> Result<Value> {
        let mut tx = self.pool.begin().await.map_err(db_error)?;
        let row = owned(&mut tx, owner, id).await?;
        let lease = body.get("lease").and_then(Value::as_str);
        let at = now();
        if body.get("release") == Some(&Value::Bool(true)) {
            if row.lease.as_deref() != lease {
                return Err(fail("제어 권한이 없어요.", 409));
            }
            stop_tx(&mut tx, id, at, "Stopped by operator").await?;
            tx.commit().await.map_err(db_error)?;
            return Ok(json!({"released":true}));
        }
        if row.lease_until > at && row.lease.as_deref() != lease {
            return Err(fail("다른 창에서 제어 중이에요.", 409));
        }
        let lease = lease
            .filter(|lease| valid_id(lease))
            .ok_or_else(|| fail("Invalid control lease", 400))?;
        if at - row.seen > 4. {
            return Err(fail("기기가 오프라인이에요.", 409));
        }
        sqlx::query("UPDATE ai_store.remote_robots SET lease=$1,lease_until=$2 WHERE id=$3")
            .bind(lease)
            .bind(at + 10.)
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_err(db_error)?;
        tx.commit().await.map_err(db_error)?;
        Ok(json!({"lease":lease,"expires_at":at+10.}))
    }
    pub async fn goal(&self, owner: &str, id: &str, body: &Value) -> Result<Value> {
        let mut tx = self.pool.begin().await.map_err(db_error)?;
        let row = owned(&mut tx, owner, id).await?;
        let at = now();
        if at - row.seen > 2.
            || row.lease_until < at
            || row.lease.as_deref() != body.get("lease").and_then(Value::as_str)
        {
            return Err(fail("연결·제어 권한을 다시 확인하세요.", 409));
        }
        let telemetry = row.telemetry.as_ref().unwrap_or(&Value::Null);
        if telemetry.get("armed") != Some(&Value::Bool(true))
            || telemetry.get("ready") != Some(&Value::Bool(true))
            || telemetry.get("emergency_stop") != Some(&Value::Bool(false))
            || telemetry.get("navigation_enabled") != Some(&Value::Bool(true))
        {
            return Err(fail(
                "라즈에서 이동을 활성화하고 지도·센서 상태를 확인하세요.",
                409,
            ));
        }
        let map = remote_protocol::grid(row.map.as_ref().ok_or_else(|| {
            fail(
                "지도가 바뀌었어요. 새 지도에서 목표를 다시 선택하세요.",
                409,
            )
        })?)?;
        if body.get("map_session").and_then(Value::as_str) != Some(&map.session)
            || body
                .get("map_version")
                .and_then(Value::as_u64)
                .is_none_or(|version| version > map.version)
        {
            return Err(fail(
                "지도가 바뀌었어요. 새 지도에서 목표를 다시 선택하세요.",
                409,
            ));
        }
        let x = body
            .get("x")
            .and_then(Value::as_f64)
            .ok_or_else(|| fail("Coordinates must be finite", 400))?;
        let y = body
            .get("y")
            .and_then(Value::as_f64)
            .ok_or_else(|| fail("Coordinates must be finite", 400))?;
        let yaw = body
            .get("yaw")
            .map(Value::as_f64)
            .unwrap_or(Some(0.))
            .filter(|value| value.is_finite() && value.abs() <= std::f64::consts::TAU)
            .ok_or_else(|| fail("Invalid goal orientation", 400))?;
        remote_protocol::free_goal(&map, x, y)?;
        let live: i64=sqlx::query_scalar("SELECT count(*) FROM ai_store.robot_commands WHERE device=$1 AND state IN('queued','dispatched','running')").bind(id).fetch_one(&mut *tx).await.map_err(db_error)?;
        if live > 0 {
            return Err(fail("현재 목표를 취소한 뒤 새 목표를 보내세요.", 409));
        }
        let command = uuid::Uuid::new_v4().to_string();
        let payload = json!({"x":x,"y":y,"yaw":yaw,"frame":map.frame,"map_session":map.session,"map_version":map.version});
        sqlx::query("INSERT INTO ai_store.robot_commands(id,device,kind,state,created,expires,lease,payload,updated) VALUES($1,$2,'navigate','queued',$3,$4,$5,$6,$3)").bind(&command).bind(id).bind(at).bind(at+15.).bind(row.lease).bind(payload).execute(&mut *tx).await.map_err(db_error)?;
        tx.commit().await.map_err(db_error)?;
        Ok(json!({"command":self.latest(id).await?}))
    }
    pub async fn stop(&self, owner: &str, id: &str) -> Result<Value> {
        let mut tx = self.pool.begin().await.map_err(db_error)?;
        owned(&mut tx, owner, id).await?;
        stop_tx(&mut tx, id, now(), "Stopped by operator").await?;
        tx.commit().await.map_err(db_error)?;
        Ok(json!({"ok":true}))
    }
    pub async fn revoke(&self, owner: &str, id: &str) -> Result<Value> {
        let mut tx = self.pool.begin().await.map_err(db_error)?;
        owned(&mut tx, owner, id).await?;
        stop_tx(&mut tx, id, now(), "Stopped by operator").await?;
        sqlx::query("UPDATE ai_store.remote_robots SET revoked=true,token_hash=NULL,pair_hash=NULL,telemetry=NULL,map=NULL,image=NULL WHERE id=$1 AND owner=$2").bind(id).bind(owner).execute(&mut *tx).await.map_err(db_error)?;
        tx.commit().await.map_err(db_error)?;
        Ok(json!({"ok":true}))
    }
    pub async fn enroll(&self, body: &Value) -> Result<Value> {
        let code = target(body, "pair_code")?;
        let session = target(body, "agent_session")?;
        if code.len() > 100 || !valid_id(session) {
            return Err(fail("Invalid pairing request", 401));
        }
        let mut tx = self.pool.begin().await.map_err(db_error)?;
        let at = now();
        let row=sqlx::query_as::<_,Device>("SELECT * FROM ai_store.remote_robots WHERE pair_hash=$1 AND pair_until>$2 AND NOT revoked FOR UPDATE").bind(hash(code)).bind(at).fetch_optional(&mut *tx).await.map_err(db_error)?.ok_or_else(||fail("연결 코드가 잘못되었거나 만료됐어요.",401))?;
        let token = format!("cbgpu_robot_{}", secret(40)?);
        sqlx::query("UPDATE ai_store.remote_robots SET pair_hash=NULL,pair_until=0,token_hash=$1,agent_session=$2,seen=0 WHERE id=$3").bind(hash(&token)).bind(session).bind(&row.id).execute(&mut *tx).await.map_err(db_error)?;
        tx.commit().await.map_err(db_error)?;
        Ok(json!({"device_id":row.id,"device_token":token,"name":row.name}))
    }
    pub async fn sync(&self, body: &Value) -> Result<Value> {
        let token = body
            .get("device_token")
            .and_then(Value::as_str)
            .filter(|token| token.starts_with("cbgpu_robot_") && token.len() <= 150)
            .ok_or_else(|| fail("Device authentication required", 401))?;
        let session = target(body, "agent_session")?;
        require(valid_id(session), "Invalid agent session")?;
        let mut t = remote_protocol::telemetry(body.get("telemetry").unwrap_or(&Value::Null))?;
        let image = match body.get("image").filter(|value| !value.is_null()) {
            Some(value) => {
                let value = value
                    .as_str()
                    .ok_or_else(|| fail("Invalid JPEG preview", 400))?;
                remote_protocol::jpeg(value)?;
                Some(value)
            }
            None => None,
        };
        let new = body
            .get("map")
            .filter(|value| !value.is_null())
            .map(remote_protocol::grid)
            .transpose()?;
        let mut tx = self.pool.begin().await.map_err(db_error)?;
        let at = now();
        let mut row = sqlx::query_as::<_, Device>(
            "SELECT * FROM ai_store.remote_robots WHERE token_hash=$1 AND NOT revoked FOR UPDATE",
        )
        .bind(hash(token))
        .fetch_optional(&mut *tx)
        .await
        .map_err(db_error)?
        .ok_or_else(|| fail("Device credential revoked or invalid", 401))?;
        sqlx::query("DELETE FROM ai_store.robot_sync_events WHERE device=$1 AND at<$2")
            .bind(&row.id)
            .bind(at - 60.)
            .execute(&mut *tx)
            .await
            .map_err(db_error)?;
        let rate: i64 =
            sqlx::query_scalar("SELECT count(*) FROM ai_store.robot_sync_events WHERE device=$1")
                .bind(&row.id)
                .fetch_one(&mut *tx)
                .await
                .map_err(db_error)?;
        if rate >= 600 {
            return Err(fail("Device sync rate exceeded", 429));
        }
        sqlx::query("INSERT INTO ai_store.robot_sync_events(device,at) VALUES($1,$2)")
            .bind(&row.id)
            .bind(at)
            .execute(&mut *tx)
            .await
            .map_err(db_error)?;
        if row.agent_session.as_deref() != Some(session) {
            if at - row.seen < 5. {
                return Err(fail("Another agent is connected", 409));
            }
            stop_tx(&mut tx, &row.id, at, "Agent session changed").await?;
            row.stop_epoch += 1;
            row.lease_until = 0.;
            row.lease = None;
        }
        let mut old = row.map.as_ref().map(remote_protocol::grid).transpose()?;
        if let Some(new) = new {
            if let Some(old) = &old {
                if new.session == old.session && new.version < old.version {
                    return Err(fail("Map version moved backwards", 409));
                }
                if new.session == old.session && new.version == old.version && new != *old {
                    return Err(fail("Map content changed without a new version", 409));
                }
                if new.session != old.session {
                    stop_tx(&mut tx, &row.id, at, "Mapping session changed").await?;
                    row.stop_epoch += 1;
                    row.lease_until = 0.;
                    row.lease = None;
                }
            }
            old = Some(new);
        }
        if old.as_ref().is_none_or(|map| {
            t.get("map_session").and_then(Value::as_str) != Some(&map.session)
                || t.get("map_version").and_then(Value::as_u64) != Some(map.version)
        }) {
            t["ready"] = json!(false);
        }
        let map = old.map(|map| serde_json::to_value(map).expect("finite map"));
        sqlx::query("UPDATE ai_store.remote_robots SET seen=$1,agent_session=$2,telemetry=$3,map=$4,image=coalesce($5,image) WHERE id=$6").bind(at).bind(session).bind(&t).bind(map).bind(image).bind(&row.id).execute(&mut *tx).await.map_err(db_error)?;
        if let Some(ack) = body.get("ack").filter(|value| !value.is_null()) {
            let id = target(ack, "id")?;
            let state = target(ack, "state")?;
            require(
                valid_id(id) && ["running", "succeeded", "failed", "cancelled"].contains(&state),
                "Invalid command acknowledgement",
            )?;
            let error = ack
                .get("error")
                .map(|value| {
                    value
                        .as_str()
                        .map(str::to_owned)
                        .unwrap_or_else(|| value.to_string())
                })
                .unwrap_or_default()
                .chars()
                .take(300)
                .collect::<String>();
            sqlx::query("UPDATE ai_store.robot_commands SET state=$1,error=$2,updated=$3,accepted_at=coalesce(accepted_at,$3) WHERE id=$4 AND device=$5 AND state IN('dispatched','running')").bind(state).bind(if error.is_empty(){None}else{Some(error)}).bind(at).bind(id).bind(&row.id).execute(&mut *tx).await.map_err(db_error)?;
        }
        sqlx::query("UPDATE ai_store.robot_commands SET state='expired',error='Goal delivery expired',updated=$1 WHERE device=$2 AND state IN('queued','dispatched') AND expires<$1").bind(at).bind(&row.id).execute(&mut *tx).await.map_err(db_error)?;
        let live = row.lease_until > at;
        // A device must independently enforce sensor/arming safety; no motion commands are executed here.
        if !live {
            sqlx::query("UPDATE ai_store.robot_commands SET state='cancelled',error='Operator control lease expired',updated=$1 WHERE device=$2 AND state IN('queued','dispatched','running')").bind(at).bind(&row.id).execute(&mut *tx).await.map_err(db_error)?;
        }
        let command = if live {
            sqlx::query_as::<_,Command>("SELECT id,kind,state,created,expires,payload,error,updated,accepted_at FROM ai_store.robot_commands WHERE device=$1 AND state IN('queued','dispatched') AND expires>$2 ORDER BY created,id LIMIT 1").bind(&row.id).bind(at).fetch_optional(&mut *tx).await.map_err(db_error)?
        } else {
            None
        };
        if let Some(command) = &command {
            sqlx::query(
                "UPDATE ai_store.robot_commands SET state='dispatched',updated=$1 WHERE id=$2",
            )
            .bind(at)
            .bind(&command.id)
            .execute(&mut *tx)
            .await
            .map_err(db_error)?;
        }
        sqlx::query("DELETE FROM ai_store.robot_commands WHERE updated<$1")
            .bind(at - 7. * 86400.)
            .execute(&mut *tx)
            .await
            .map_err(db_error)?;
        tx.commit().await.map_err(db_error)?;
        Ok(
            json!({"server_time":at,"operator_live":live,"stop_epoch":row.stop_epoch,"command":command.map(|command|json!({"id":command.id,"expires_at":command.expires,"payload":command.payload}))}),
        )
    }
}
