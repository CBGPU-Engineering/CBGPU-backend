use crate::{Identity, Principal, Result, fail};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde_json::Value;
#[async_trait::async_trait]
pub trait Sessions: Send + Sync {
    async fn active(&self, session: uuid::Uuid, subject: uuid::Uuid) -> Result<Option<bool>>;
}
pub struct PgSessions {
    pub pool: sqlx::PgPool,
}
#[async_trait::async_trait]
impl Sessions for PgSessions {
    async fn active(&self, session: uuid::Uuid, subject: uuid::Uuid) -> Result<Option<bool>> {
        let mut tx = self.pool.begin().await.map_err(|_| {
            fail(
                503,
                "authentication_unavailable",
                "Fresh session verification unavailable",
            )
        })?;
        let result = async {
            sqlx::query("SELECT set_config('request.jwt.claims',$1,true)")
                .bind(
                    serde_json::json!({"role":"authenticated","sub":subject,"session_id":session})
                        .to_string(),
                )
                .execute(&mut *tx)
                .await?;
            sqlx::query_as::<_, (bool, bool)>(
                "SELECT valid,approved FROM cbgpu_contests_auth.session_status($1,$2)",
            )
            .bind(subject)
            .bind(session)
            .fetch_optional(&mut *tx)
            .await
        }
        .await;
        let rollback = tx.rollback().await;
        let row = result.map_err(|_| {
            fail(
                503,
                "authentication_unavailable",
                "Fresh session verification unavailable",
            )
        })?;
        rollback.map_err(|_| {
            fail(
                503,
                "authentication_unavailable",
                "Fresh session verification unavailable",
            )
        })?;
        Ok(row.and_then(|(valid, approved)| valid.then_some(approved)))
    }
}
pub struct Auth {
    pub origin: String,
    pub anon_key: String,
    pub sessions: std::sync::Arc<dyn Sessions>,
    pub client: reqwest::Client,
    pub account_v2: bool,
}
#[async_trait::async_trait]
impl Identity for Auth {
    async fn authenticate(&self, token: &str) -> Result<Principal> {
        let claims = claims(token)?;
        let subject = claims["sub"]
            .as_str()
            .and_then(|v| uuid::Uuid::parse_str(v).ok())
            .ok_or_else(|| fail(401, "unauthorized", "Access JWT subject missing"))?;
        let session = claims["session_id"]
            .as_str()
            .and_then(|v| uuid::Uuid::parse_str(v).ok())
            .ok_or_else(|| fail(401, "unauthorized", "Use a current signed-in CBGPU session"))?;
        if claims["role"] != "authenticated"
            || claims["exp"]
                .as_f64()
                .is_none_or(|exp| !exp.is_finite() || exp <= now())
        {
            return Err(fail(
                401,
                "unauthorized",
                "Access JWT expired or has the wrong role",
            ));
        }
        let response = self
            .client
            .get(format!("{}/auth/v1/user", self.origin))
            .header("apikey", &self.anon_key)
            .bearer_auth(token)
            .send()
            .await
            .map_err(|_| {
                fail(
                    503,
                    "authentication_unavailable",
                    "CBGPU authentication unavailable",
                )
            })?;
        if !response.status().is_success() {
            return Err(fail(
                401,
                "unauthorized",
                "CBGPU access JWT was not accepted",
            ));
        }
        let user: Value = response.json().await.map_err(|_| {
            fail(
                503,
                "authentication_unavailable",
                "Invalid authentication response",
            )
        })?;
        if user["id"]
            .as_str()
            .and_then(|v| uuid::Uuid::parse_str(v).ok())
            != Some(subject)
        {
            return Err(fail(
                401,
                "unauthorized",
                "Verified account does not match JWT subject",
            ));
        }
        let legacy_permission = self
            .sessions
            .active(session, subject)
            .await?
            .ok_or_else(|| {
                fail(
                    401,
                    "session_revoked",
                    "Session expired, signed out, or account is unavailable",
                )
            })?;
        let may_register = if self.account_v2 {
            let response = self
                .client
                .post(format!(
                    "{}/rest/v1/rpc/cbgpu_can_use_resource",
                    self.origin
                ))
                .header("apikey", &self.anon_key)
                .bearer_auth(token)
                .json(&serde_json::json!({"resource":"contests","target_school":"cbsh"}))
                .send()
                .await
                .map_err(|_| {
                    fail(
                        503,
                        "authorization_unavailable",
                        "Contest permission unavailable",
                    )
                })?;
            if !response.status().is_success() {
                return Err(fail(
                    503,
                    "authorization_unavailable",
                    "Contest permission unavailable",
                ));
            }
            response.json::<Value>().await.map_err(|_| {
                fail(
                    503,
                    "authorization_unavailable",
                    "Contest permission unavailable",
                )
            })? == true
        } else {
            legacy_permission
        };
        Ok(Principal {
            subject,
            may_register,
        })
    }
}
fn now() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}
fn claims(token: &str) -> Result<Value> {
    let pieces: Vec<_> = token.split('.').collect();
    if token.len() > 16384 || pieces.len() != 3 {
        return Err(fail(401, "unauthorized", "Use a CBGPU access JWT"));
    }
    let bytes = URL_SAFE_NO_PAD
        .decode(pieces[1])
        .map_err(|_| fail(401, "unauthorized", "Invalid access JWT"))?;
    serde_json::from_slice(&bytes).map_err(|_| fail(401, "unauthorized", "Invalid access JWT"))
}
