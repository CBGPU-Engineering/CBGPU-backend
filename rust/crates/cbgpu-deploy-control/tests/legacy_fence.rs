#![cfg(unix)]
//! New isolated SQLite only. No operating DB, jobs, Docker or services.
use cbgpu_deploy_control::{crypto::Cipher, legacy_fence};
use serde_json::{Value, json};
use sqlx::{Connection, Row, SqliteConnection, sqlite::SqliteConnectOptions};
use std::{fs, os::unix::fs::PermissionsExt, path::PathBuf};
struct Fixture {
    _dir: tempfile::TempDir,
    database: PathBuf,
    key: PathBuf,
    receipt: PathBuf,
}
impl Fixture {
    async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let database = dir.path().join("deploy.sqlite");
        let key = dir.path().join("frontend-deploy-key");
        let receipt = dir.path().join("fence.json");
        fs::write(&key, [8u8; 32]).unwrap();
        fs::set_permissions(&key, fs::Permissions::from_mode(0o600)).unwrap();
        let mut c = SqliteConnection::connect_with(
            &SqliteConnectOptions::new()
                .filename(&database)
                .create_if_missing(true),
        )
        .await
        .unwrap();
        sqlx::raw_sql("CREATE TABLE projects(id TEXT PRIMARY KEY,owner TEXT,secrets TEXT,settings TEXT);CREATE TABLE deployments(id TEXT PRIMARY KEY,project_id TEXT,status TEXT);CREATE TABLE compose_states(project_id TEXT PRIMARY KEY,status TEXT,logs TEXT);CREATE TRIGGER application_history AFTER UPDATE ON projects BEGIN SELECT 1; END;").execute(&mut c).await.unwrap();
        let secret = Cipher::new(&[8; 32])
            .unwrap()
            .seal(&json!({"env":{"fixture":"private"}}))
            .unwrap();
        sqlx::query("INSERT INTO projects VALUES('project','original-owner',?,'original')")
            .bind(secret)
            .execute(&mut c)
            .await
            .unwrap();
        sqlx::query("INSERT INTO deployments VALUES('old','project','ready')")
            .execute(&mut c)
            .await
            .unwrap();
        sqlx::query("INSERT INTO compose_states VALUES('server','ready','original')")
            .execute(&mut c)
            .await
            .unwrap();
        c.close().await.unwrap();
        Self {
            _dir: dir,
            database,
            key,
            receipt,
        }
    }
    async fn db(&self) -> SqliteConnection {
        SqliteConnection::connect_with(
            &SqliteConnectOptions::new()
                .filename(&self.database)
                .create_if_missing(false)
                .busy_timeout(std::time::Duration::from_secs(5)),
        )
        .await
        .unwrap()
    }
    async fn own_triggers(&self) -> i64 {
        let mut c = self.db().await;
        sqlx::query_scalar("SELECT count(*) FROM sqlite_master WHERE type='trigger' AND name LIKE 'cbgpu_handoff_%'").fetch_one(&mut c).await.unwrap()
    }
    fn manifest(&self) -> Value {
        let env: Value = serde_json::from_slice(&fs::read(&self.receipt).unwrap()).unwrap();
        Cipher::new(&[8; 32])
            .unwrap()
            .unseal(env["sealed_receipt"].as_str().unwrap())
            .unwrap()
    }
}
#[tokio::test]
async fn busy_refusal_and_default_plan_preserve_every_row_and_create_no_receipt() {
    let f = Fixture::new().await;
    let plan = legacy_fence::plan(&f.database, &f.key).await.unwrap();
    assert_eq!(plan["changed"], false);
    assert_eq!(f.own_triggers().await, 0);
    let mut c = f.db().await;
    sqlx::query("INSERT INTO deployments VALUES('active','project','queued')")
        .execute(&mut c)
        .await
        .unwrap();
    assert_eq!(
        legacy_fence::apply(&f.database, &f.key, &f.receipt)
            .await
            .unwrap_err()
            .status,
        409
    );
    assert!(!f.receipt.exists());
    assert_eq!(f.own_triggers().await, 0);
    sqlx::query("DELETE FROM deployments WHERE id='active'")
        .execute(&mut c)
        .await
        .unwrap();
    sqlx::query("UPDATE compose_states SET status='building'")
        .execute(&mut c)
        .await
        .unwrap();
    assert!(
        legacy_fence::apply(&f.database, &f.key, &f.receipt)
            .await
            .is_err()
    );
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT status FROM compose_states")
            .fetch_one(&mut c)
            .await
            .unwrap(),
        "building"
    );
    assert!(!f.receipt.exists());
    assert_eq!(f.own_triggers().await, 0)
}
#[tokio::test]
async fn fence_blocks_all_new_launches_but_preserves_reads_settings_and_ready_workloads() {
    let f = Fixture::new().await;
    let mut c = f.db().await;
    let secret: String = sqlx::query_scalar("SELECT secrets FROM projects")
        .fetch_one(&mut c)
        .await
        .unwrap();
    legacy_fence::apply(&f.database, &f.key, &f.receipt)
        .await
        .unwrap();
    assert_eq!(f.own_triggers().await, 3);
    assert_eq!(
        fs::metadata(&f.receipt).unwrap().permissions().mode() & 0o077,
        0
    );
    let m = f.manifest();
    assert_eq!(
        m["database"].as_str().unwrap(),
        f.database.to_str().unwrap()
    );
    assert_eq!(m["triggers"].as_array().unwrap().len(), 3);
    assert!(
        !fs::read_to_string(&f.receipt)
            .unwrap()
            .contains("CREATE TRIGGER")
    );
    for query in [
        "INSERT INTO deployments VALUES('new','project','queued')",
        "INSERT INTO compose_states VALUES('new','ready','new')",
        "UPDATE compose_states SET status='building'",
        "UPDATE compose_states SET status='queued'",
    ] {
        assert!(sqlx::query(query).execute(&mut c).await.is_err())
    }
    sqlx::query("UPDATE projects SET settings='safe edit'")
        .execute(&mut c)
        .await
        .unwrap();
    sqlx::query("UPDATE compose_states SET logs='still ready'")
        .execute(&mut c)
        .await
        .unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT owner FROM projects")
            .fetch_one(&mut c)
            .await
            .unwrap(),
        "original-owner"
    );
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT secrets FROM projects")
            .fetch_one(&mut c)
            .await
            .unwrap(),
        secret
    );
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT status FROM deployments")
            .fetch_one(&mut c)
            .await
            .unwrap(),
        "ready"
    );
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT status FROM compose_states")
            .fetch_one(&mut c)
            .await
            .unwrap(),
        "ready"
    );
    // The new supervisor performs additive SQLite startup columns while fenced.
    // These must not erase or change the exact guard definitions/receipt contract.
    sqlx::raw_sql("ALTER TABLE deployments ADD COLUMN requested_by TEXT;ALTER TABLE compose_states ADD COLUMN access_state TEXT;UPDATE compose_states SET access_state='ALLOW';").execute(&mut c).await.unwrap();
    let removed = legacy_fence::remove(&f.database, &f.key, &f.receipt)
        .await
        .unwrap();
    assert_eq!(removed["owned_triggers_removed"], 3);
    assert_eq!(f.own_triggers().await, 0);
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM sqlite_master WHERE type='trigger' AND name='application_history'"
        )
        .fetch_one(&mut c)
        .await
        .unwrap(),
        1
    );
    sqlx::query("INSERT INTO deployments(id,project_id,status) VALUES('after','project','queued')")
        .execute(&mut c)
        .await
        .unwrap();
    assert_eq!(fs::read(&f.key).unwrap(), [8u8; 32]);
    assert_eq!(
        legacy_fence::remove(&f.database, &f.key, &f.receipt)
            .await
            .unwrap()["state"],
        "already_absent"
    )
}
#[tokio::test]
async fn tampered_receipt_wrong_key_and_other_database_never_remove_any_guard() {
    let f = Fixture::new().await;
    legacy_fence::apply(&f.database, &f.key, &f.receipt)
        .await
        .unwrap();
    let original = fs::read(&f.receipt).unwrap();
    let mut envelope: Value = serde_json::from_slice(&original).unwrap();
    let mut value = envelope["sealed_receipt"].as_str().unwrap().to_owned();
    value.replace_range(2..3, "!");
    envelope["sealed_receipt"] = json!(value);
    fs::write(&f.receipt, serde_json::to_vec(&envelope).unwrap()).unwrap();
    assert!(
        legacy_fence::remove(&f.database, &f.key, &f.receipt)
            .await
            .is_err()
    );
    assert_eq!(f.own_triggers().await, 3);
    fs::write(&f.receipt, original).unwrap();
    let wrong = f.key.with_file_name("wrong-key");
    fs::write(&wrong, [9u8; 32]).unwrap();
    fs::set_permissions(&wrong, fs::Permissions::from_mode(0o600)).unwrap();
    assert!(
        legacy_fence::remove(&f.database, &wrong, &f.receipt)
            .await
            .is_err()
    );
    let other = Fixture::new().await;
    assert!(
        legacy_fence::remove(&other.database, &other.key, &f.receipt)
            .await
            .is_err()
    );
    assert_eq!(f.own_triggers().await, 3);
    assert_eq!(other.own_triggers().await, 0)
}
#[tokio::test]
async fn modified_own_trigger_refuses_all_drops_and_preserves_third_party_ddl() {
    let f = Fixture::new().await;
    legacy_fence::apply(&f.database, &f.key, &f.receipt)
        .await
        .unwrap();
    let name = f.manifest()["triggers"][0]["name"]
        .as_str()
        .unwrap()
        .to_owned();
    let mut c = f.db().await;
    sqlx::query(&format!("DROP TRIGGER {name}"))
        .execute(&mut c)
        .await
        .unwrap();
    sqlx::query(&format!("CREATE TRIGGER {name} BEFORE INSERT ON deployments BEGIN SELECT RAISE(ABORT,'manual replacement'); END")).execute(&mut c).await.unwrap();
    assert!(
        legacy_fence::remove(&f.database, &f.key, &f.receipt)
            .await
            .is_err()
    );
    assert_eq!(f.own_triggers().await, 3);
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM sqlite_master WHERE type='trigger' AND name='application_history'"
        )
        .fetch_one(&mut c)
        .await
        .unwrap(),
        1
    )
}
#[tokio::test]
async fn existing_receipt_is_never_overwritten() {
    let f = Fixture::new().await;
    fs::write(&f.receipt, b"existing operator file").unwrap();
    assert!(
        legacy_fence::apply(&f.database, &f.key, &f.receipt)
            .await
            .is_err()
    );
    assert_eq!(fs::read(&f.receipt).unwrap(), b"existing operator file");
    assert_eq!(f.own_triggers().await, 0)
}
#[tokio::test]
async fn concurrent_writer_committed_before_lock_is_detected_not_interrupted() {
    let f = Fixture::new().await;
    let mut writer = f.db().await;
    let mut tx = writer.begin_with("BEGIN IMMEDIATE").await.unwrap();
    sqlx::query("INSERT INTO deployments VALUES('race','project','building')")
        .execute(&mut *tx)
        .await
        .unwrap();
    let database = f.database.clone();
    let key = f.key.clone();
    let receipt = f.receipt.clone();
    let fence = tokio::spawn(async move { legacy_fence::apply(&database, &key, &receipt).await });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    tx.commit().await.unwrap();
    assert!(fence.await.unwrap().is_err());
    assert_eq!(f.own_triggers().await, 0);
    assert!(!f.receipt.exists());
    let row = sqlx::query("SELECT status FROM deployments WHERE id='race'")
        .fetch_one(&mut writer)
        .await
        .unwrap();
    assert_eq!(row.get::<String, _>("status"), "building")
}
