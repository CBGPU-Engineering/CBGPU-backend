//! Explicit isolated test: cargo test -p cbgpu-robotics --test postgres -- --ignored
//! Creates and removes its own disposable PostgreSQL container; never connects to production.
use cbgpu_robotics::{
    Result, fail, migrate, now,
    planning::{JobBackend, Robotics},
    remote::RemoteRobots,
};
use serde_json::{Value, json};
use sqlx::{PgPool, postgres::PgPoolOptions};
use std::{
    collections::BTreeMap,
    process::Command,
    sync::{Arc, Mutex},
};
struct Container(String);
impl Drop for Container {
    fn drop(&mut self) {
        let _ = Command::new("docker").args(["rm", "-f", &self.0]).output();
    }
}
async fn database() -> (Container, PgPool) {
    let name = format!("cbgpu-robotics-test-{}", uuid::Uuid::new_v4());
    let result = Command::new("docker")
        .args([
            "run",
            "--rm",
            "-d",
            "--name",
            &name,
            "-e",
            "POSTGRES_PASSWORD=synthetic-robotics-tests",
            "-e",
            "POSTGRES_DB=robotics_contract",
            "-p",
            "127.0.0.1::5432",
            "postgres:17-alpine",
        ])
        .output()
        .expect("docker available");
    assert!(
        result.status.success(),
        "could not start isolated test database"
    );
    let guard = Container(name.clone());
    let binding = Command::new("docker")
        .args(["port", &name, "5432/tcp"])
        .output()
        .unwrap();
    let address = String::from_utf8(binding.stdout).unwrap();
    let port = address.trim().rsplit(':').next().unwrap();
    let url =
        format!("postgres://postgres:synthetic-robotics-tests@127.0.0.1:{port}/robotics_contract");
    let mut pool = None;
    for _ in 0..100 {
        if let Ok(value) = PgPoolOptions::new()
            .max_connections(10)
            .acquire_timeout(std::time::Duration::from_secs(1))
            .connect(&url)
            .await
        {
            pool = Some(value);
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    let pool = pool.expect("isolated PostgreSQL ready");
    migrate(&pool).await.unwrap();
    migrate(&pool).await.unwrap();
    pool.close().await;
    let pool = PgPoolOptions::new()
        .max_connections(10)
        .after_connect(|connection, _| {
            Box::pin(async move {
                sqlx::query("SET ROLE cbgpu_ai").execute(connection).await?;
                Ok(())
            })
        })
        .connect(&url)
        .await
        .unwrap();
    (guard, pool)
}
fn map() -> Value {
    json!({"width":4,"height":4,"resolution":0.5,"origin":{"x":0,"y":0,"yaw":0},"frame":"map","session":"mapping-1","version":1,"rle":[0,8,-1,4,100,4]})
}
fn caps() -> Value {
    json!([{"name":"navigate_to","description":"Navigate to a known waypoint","parameters":{"type":"object","properties":{"waypoint":{"type":"string","maxLength":32,"enum":["desk","charging_station"]}},"required":["waypoint"],"additionalProperties":false},"max_duration_s":10}])
}
fn plan() -> Value {
    json!({"summary":"Move to charging station","needs_clarification":false,"question":"","steps":[{"capability":"navigate_to","arguments":{"waypoint":"charging_station"},"duration_s":8}]})
}
fn sync(token: &str, session: &str) -> Value {
    json!({"device_token":token,"agent_session":session,"telemetry":{"armed":true,"ready":true,"emergency_stop":false,"navigation_enabled":true,"pose":{"x":0.25,"y":0.25,"yaw":0},"map_session":"mapping-1","map_version":1,"sensor_age_s":0},"map":map()})
}
fn goal(lease: &str) -> Value {
    json!({"lease":lease,"x":0.75,"y":0.75,"yaw":0,"map_session":"mapping-1","map_version":1})
}
#[derive(Default)]
struct Jobs(Mutex<BTreeMap<String, (String, Value)>>);
#[async_trait::async_trait]
impl JobBackend for Jobs {
    async fn enqueue(&self, owner: &str, _: &str, input: Value, _: &str) -> Result<Value> {
        let id = uuid::Uuid::new_v4().to_string();
        let job = json!({"id":id,"state":"queued","input":input,"progress":"","error":null});
        self.0
            .lock()
            .unwrap()
            .insert(id, (owner.into(), job.clone()));
        Ok(job)
    }
    async fn job(&self, owner: &str, id: &str) -> Result<Value> {
        self.0
            .lock()
            .unwrap()
            .get(id)
            .filter(|(who, _)| who == owner)
            .map(|(_, job)| job.clone())
            .ok_or_else(|| fail("Job not found", 404))
    }
    async fn cancel(&self, owner: &str, id: &str) -> Result<Value> {
        self.job(owner, id).await?;
        let mut data = self.0.lock().unwrap();
        let job = &mut data.get_mut(id).unwrap().1;
        if ["queued", "running", "paused"].contains(&job["state"].as_str().unwrap_or("")) {
            job["state"] = json!("cancelled")
        }
        Ok(json!({"ok":true}))
    }
}
impl Jobs {
    fn result(&self, id: &str, plan: Value) {
        self.0.lock().unwrap().get_mut(id).unwrap().1 = json!({"id":id,"state":"succeeded","result":{"choices":[{"message":{"tool_calls":[{"function":{"name":"propose_robot_plan","arguments":plan.to_string()}}]}}]}})
    }
}
async fn import_fixture(pool: &PgPool) {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("queue.sqlite");
    let sqlite = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(
            sqlx::sqlite::SqliteConnectOptions::new()
                .filename(&path)
                .create_if_missing(true),
        )
        .await
        .unwrap();
    sqlx::raw_sql("CREATE TABLE remote_robots(id TEXT PRIMARY KEY,owner TEXT NOT NULL,name TEXT NOT NULL,created REAL NOT NULL,revoked INTEGER DEFAULT 0,pair_hash TEXT,pair_until REAL,token_hash TEXT,agent_session TEXT,seen REAL DEFAULT 0,lease TEXT,lease_until REAL DEFAULT 0,telemetry TEXT,map TEXT,image TEXT,stop_epoch INTEGER DEFAULT 0);CREATE TABLE robot_commands(id TEXT PRIMARY KEY,device TEXT NOT NULL,kind TEXT NOT NULL,state TEXT NOT NULL,created REAL NOT NULL,expires REAL NOT NULL,lease TEXT,payload TEXT,error TEXT,updated REAL NOT NULL);CREATE TABLE robot_plans(id TEXT PRIMARY KEY,owner TEXT NOT NULL,robot TEXT NOT NULL,capabilities TEXT NOT NULL,observed REAL NOT NULL,mission TEXT,error TEXT);").execute(&sqlite).await.unwrap();
    let id = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
    let owner = "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb";
    sqlx::query("INSERT INTO remote_robots(id,owner,name,created,pair_hash,pair_until,token_hash,agent_session,seen,lease,lease_until,map,stop_epoch) VALUES(?,?,?,1.25,'hash-pair',10.25,'hash-token','old-session',1.5,'old-lease',3.5,?,7)").bind(id).bind(owner).bind("Retained robot").bind(map().to_string()).execute(&sqlite).await.unwrap();
    sqlx::query("INSERT INTO robot_commands(id,device,kind,state,created,expires,lease,payload,updated) VALUES('cccccccc-cccc-4ccc-8ccc-cccccccccccc',?,'navigate','cancelled',1.25,2.5,'old-lease','{\"x\":1,\"y\":2}',1.5)").bind(id).execute(&sqlite).await.unwrap();
    sqlx::query("INSERT INTO robot_plans(id,owner,robot,capabilities,observed,mission) VALUES('dddddddd-dddd-4ddd-8ddd-dddddddddddd',?,'old-robot',?,1.25,'{\"requires_local_approval\":true}')").bind(owner).bind(caps().to_string()).execute(&sqlite).await.unwrap();
    sqlx::query("ALTER TABLE robot_plans ADD COLUMN unknown_future_field TEXT")
        .execute(&sqlite)
        .await
        .unwrap();
    assert!(
        cbgpu_robotics::import::import_read_only(pool, &path)
            .await
            .is_err()
    );
    let empty: i64 = sqlx::query_scalar("SELECT count(*) FROM ai_store.remote_robots")
        .fetch_one(pool)
        .await
        .unwrap();
    assert_eq!(
        empty, 0,
        "failed import must roll back all previously copied tables"
    );
    sqlx::query("ALTER TABLE robot_plans DROP COLUMN unknown_future_field")
        .execute(&sqlite)
        .await
        .unwrap();
    sqlite.close().await;
    let original = std::fs::read(&path).unwrap();
    let report = cbgpu_robotics::import::import_read_only(pool, &path)
        .await
        .unwrap();
    assert_eq!(report["verified"], true);
    assert_eq!(
        report["tables"],
        json!({"remote_robots":1,"robot_commands":1,"robot_plans":1})
    );
    assert_eq!(
        std::fs::read(&path).unwrap(),
        original,
        "source snapshot must remain byte-for-byte unchanged"
    );
    let hashes: (String, String, i64) = sqlx::query_as(
        "SELECT pair_hash,token_hash,stop_epoch FROM ai_store.remote_robots WHERE id=$1",
    )
    .bind(id)
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(hashes, ("hash-pair".into(), "hash-token".into(), 7));
    assert_eq!(
        cbgpu_robotics::import::import_read_only(pool, &path)
            .await
            .unwrap_err()
            .status,
        409
    );
}
#[tokio::test]
#[ignore = "requires Docker; creates a disposable isolated PostgreSQL instance"]
async fn native_robotics_postgres_owner_session_lease_stop_and_plans() {
    let (_container, pool) = database().await;
    import_fixture(&pool).await;
    let remote = RemoteRobots::new(pool.clone());
    let owner = uuid::Uuid::new_v4().to_string();
    let other = uuid::Uuid::new_v4().to_string();
    let created = remote.create(&owner, "Test Pi").await.unwrap();
    let id = created["device"]["id"].as_str().unwrap();
    let code = created["pair_code"].as_str().unwrap();
    let session = uuid::Uuid::new_v4().to_string();
    let paired = remote
        .enroll(&json!({"pair_code":code,"agent_session":session}))
        .await
        .unwrap();
    let token = paired["device_token"].as_str().unwrap();
    let lease = uuid::Uuid::new_v4().to_string();
    assert_eq!(
        remote
            .enroll(&json!({"pair_code":code,"agent_session":session}))
            .await
            .unwrap_err()
            .status,
        401
    );
    for result in [
        remote.state(&other, id).await,
        remote.stop(&other, id).await,
        remote.revoke(&other, id).await,
        remote.control(&other, id, &json!({"lease":lease})).await,
    ] {
        assert_eq!(result.unwrap_err().status, 404)
    }
    assert_eq!(remote.list(&other).await.unwrap(), json!([]));
    let public = remote.list(&owner).await.unwrap().to_string();
    assert!(!public.contains("token_hash"));
    assert!(!public.contains(code));
    assert!(!public.contains(token));
    remote.sync(&sync(token, &session)).await.unwrap();
    remote
        .control(&owner, id, &json!({"lease":lease}))
        .await
        .unwrap();
    assert_eq!(
        remote
            .control(
                &owner,
                id,
                &json!({"lease":uuid::Uuid::new_v4().to_string()})
            )
            .await
            .unwrap_err()
            .status,
        409
    );
    for patch in [
        json!({"x":0.25,"y":1.25}),
        json!({"x":0.25,"y":1.75}),
        json!({"x":100}),
        json!({"map_session":"old"}),
        json!({"map_version":100}),
    ] {
        let mut body = goal(&lease);
        for (k, v) in patch.as_object().unwrap() {
            body[k] = v.clone()
        }
        assert!(remote.goal(&owner, id, &body).await.is_err())
    }
    let first = goal(&lease);
    let second = first.clone();
    let (a, b) = tokio::join!(
        remote.goal(&owner, id, &first),
        remote.goal(&owner, id, &second)
    );
    assert_ne!(a.is_ok(), b.is_ok());
    let command = a.or(b).unwrap()["command"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let reply = remote.sync(&sync(token, &session)).await.unwrap();
    assert_eq!(reply["command"]["id"], command);
    let mut ack = sync(token, &session);
    ack["ack"] = json!({"id":command,"state":"running"});
    remote.sync(&ack).await.unwrap();
    assert_eq!(
        remote.state(&owner, id).await.unwrap()["command"]["state"],
        "running"
    );
    remote.stop(&owner, id).await.unwrap();
    let reply = remote.sync(&sync(token, &session)).await.unwrap();
    assert_eq!(reply["operator_live"], false);
    assert!(reply["stop_epoch"].as_i64().unwrap() > 0);
    assert!(reply["command"].is_null());
    let mut duplicate = sync(token, &session);
    duplicate["agent_session"] = json!(uuid::Uuid::new_v4().to_string());
    assert_eq!(remote.sync(&duplicate).await.unwrap_err().status, 409);
    let mut changed = sync(token, &session);
    changed["map"]["rle"] = json!([0, 16]);
    assert_eq!(remote.sync(&changed).await.unwrap_err().status, 409);
    remote
        .control(&owner, id, &json!({"lease":lease}))
        .await
        .unwrap();
    let mut stale = sync(token, &session);
    stale["telemetry"]["sensor_age_s"] = json!(9);
    remote.sync(&stale).await.unwrap();
    assert!(remote.goal(&owner, id, &goal(&lease)).await.is_err());
    remote.sync(&sync(token, &session)).await.unwrap();
    remote.goal(&owner, id, &goal(&lease)).await.unwrap();
    sqlx::query("UPDATE ai_store.remote_robots SET lease_until=0 WHERE id=$1")
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
    let reply = remote.sync(&sync(token, &session)).await.unwrap();
    assert_eq!(reply["operator_live"], false);
    assert_eq!(
        remote.state(&owner, id).await.unwrap()["command"]["state"],
        "cancelled"
    );
    // Mapping-session changes stop live goals and clear the operator lease.
    remote
        .control(&owner, id, &json!({"lease":lease}))
        .await
        .unwrap();
    remote.goal(&owner, id, &goal(&lease)).await.unwrap();
    let mut remapped = sync(token, &session);
    remapped["map"]["session"] = json!("mapping-2");
    remapped["telemetry"]["map_session"] = json!("mapping-2");
    let changed = remote.sync(&remapped).await.unwrap();
    assert_eq!(changed["operator_live"], false);
    assert_eq!(
        remote.state(&owner, id).await.unwrap()["command"]["state"],
        "cancelled"
    );
    // Pairing rotation immediately revokes the old outbound device credential.
    let new_pair = remote.re_pair(&owner, id).await.unwrap();
    assert!(remote.sync(&remapped).await.is_err());
    let replacement = remote
        .enroll(&json!({"pair_code":new_pair["pair_code"],"agent_session":session}))
        .await
        .unwrap();
    let new_token = replacement["device_token"].as_str().unwrap();
    remote.sync(&sync(new_token, &session)).await.unwrap();
    remote.revoke(&owner, id).await.unwrap();
    assert_eq!(
        remote
            .sync(&sync(new_token, &session))
            .await
            .unwrap_err()
            .status,
        401
    );
    // Enforce one-time pairing atomically across independently concurrent requests.
    let another = remote.create(&owner, "Concurrent Pi").await.unwrap();
    let enrollment = json!({"pair_code":another["pair_code"],"agent_session":session});
    let (a, b) = tokio::join!(remote.enroll(&enrollment), remote.enroll(&enrollment));
    assert_ne!(a.is_ok(), b.is_ok());
    let jobs = Arc::new(Jobs::default());
    let planner = Robotics::new(pool.clone(), jobs.clone());
    let request = json!({"instruction":"Go to charging station","robot_id":"test-robot","capabilities":caps(),"observations":{"location":"desk"},"observed_at":now()});
    let job = planner.create(&owner, &request).await.unwrap();
    let id = job["id"].as_str().unwrap();
    assert_eq!(job["state"], "queued");
    assert_eq!(planner.get(&other, id).await.unwrap_err().status, 404);
    assert_eq!(planner.cancel(&other, id).await.unwrap_err().status, 404);
    jobs.result(id, plan());
    let mission = planner.get(&owner, id).await.unwrap()["mission"].clone();
    assert_eq!(mission["requires_local_approval"], true);
    assert_eq!(mission, planner.get(&owner, id).await.unwrap()["mission"]);
    // Plan validation failures are persisted and can never later become valid missions.
    let invalid = planner.create(&owner, &request).await.unwrap();
    let bad_id = invalid["id"].as_str().unwrap();
    let mut bad = plan();
    bad["steps"][0]["capability"] = json!("shell");
    jobs.result(bad_id, bad);
    assert_eq!(
        planner.get(&owner, bad_id).await.unwrap()["state"],
        "failed"
    );
    jobs.result(bad_id, plan());
    assert_eq!(
        planner.get(&owner, bad_id).await.unwrap()["state"],
        "failed"
    );
    let expired = planner.create(&owner, &request).await.unwrap();
    let expired_id = expired["id"].as_str().unwrap();
    jobs.result(expired_id, plan());
    sqlx::query("UPDATE ai_store.robot_plans SET observed=0 WHERE id=$1")
        .bind(expired_id)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        planner.get(&owner, expired_id).await.unwrap()["state"],
        "failed"
    );
    let clarification = planner.create(&owner, &request).await.unwrap();
    let clarification_id = clarification["id"].as_str().unwrap();
    jobs.result(clarification_id,json!({"summary":"Need destination","needs_clarification":true,"question":"Which waypoint?","steps":[]}));
    assert_eq!(
        planner.get(&owner, clarification_id).await.unwrap()["mission"]["plan"]["steps"],
        json!([])
    );
    pool.close().await;
}
