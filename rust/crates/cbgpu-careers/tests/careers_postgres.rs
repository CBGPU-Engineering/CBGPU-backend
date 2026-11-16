use axum::{body::Body, http::Request};
use cbgpu_careers::{App, issue_token, router};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use std::sync::Arc;
use tower::ServiceExt;

struct DockerFixture(String);
impl Drop for DockerFixture {
    fn drop(&mut self) {
        let _ = std::process::Command::new("docker")
            .args(["rm", "-f", &self.0])
            .output();
    }
}
fn docker(args: &[&str]) -> String {
    let result = std::process::Command::new("docker")
        .args(args)
        .output()
        .expect("Docker required for explicitly enabled fixture");
    assert!(
        result.status.success(),
        "Isolated PostgreSQL fixture command failed"
    );
    String::from_utf8(result.stdout).unwrap().trim().to_owned()
}
async fn database_fixture() -> (String, Option<DockerFixture>) {
    if let Ok(url) = std::env::var("CBGPU_CAREERS_TEST_DATABASE_URL") {
        return (url, None);
    }
    assert_eq!(
        std::env::var("CBGPU_CAREERS_RUN_DOCKER_TEST").as_deref(),
        Ok("1"),
        "Explicit isolated database URL or Docker opt-in required"
    );
    let name = format!("cbgpu-rust-careers-test-{:016x}", rand::random::<u64>());
    docker(&[
        "run",
        "-d",
        "--name",
        &name,
        "--memory",
        "384m",
        "--cpus",
        "1",
        "--tmpfs",
        "/var/lib/postgresql/data:rw,size=192m",
        "-e",
        "POSTGRES_PASSWORD=isolated-test-only",
        "-e",
        "POSTGRES_DB=cbgpu_careers_test",
        "-p",
        "127.0.0.1::5432",
        "postgres:17-alpine",
    ]);
    let guard = DockerFixture(name.clone());
    let port = docker(&["port", &name, "5432/tcp"]);
    let port = port.rsplit(':').next().unwrap();
    let admin_url =
        format!("postgresql://postgres:isolated-test-only@127.0.0.1:{port}/cbgpu_careers_test");
    let mut admin = None;
    for _ in 0..80 {
        if let Ok(pool) = sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .acquire_timeout(std::time::Duration::from_secs(1))
            .connect(&admin_url)
            .await
        {
            admin = Some(pool);
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }
    let admin = admin.expect("Isolated PostgreSQL readiness failed");
    sqlx::raw_sql("CREATE ROLE cbgpu_runtime LOGIN PASSWORD 'isolated-operations-only'; CREATE ROLE cbgpu_careers LOGIN NOINHERIT PASSWORD 'isolated-careers-only';").execute(&admin).await.unwrap();
    for migration in [
        include_str!("../../../migrations/20261116013020_rust_operations_core.sql"),
        include_str!("../../../migrations/20261116034058_rust_careers.sql"),
    ] {
        sqlx::raw_sql(migration).execute(&admin).await.unwrap();
    }
    admin.close().await;
    (
        format!(
            "postgresql://cbgpu_careers:isolated-careers-only@127.0.0.1:{port}/cbgpu_careers_test"
        ),
        Some(guard),
    )
}

async fn submit(app: App, token: String) -> (u16, Value) {
    let data=json!({"role":"frontend","name":"Synthetic contract applicant","email":"contract@example.invalid","motivation":"Synthetic motivation for the isolated database test only.","consent":true,"token":token}).to_string();
    let request = Request::builder()
        .method("POST")
        .uri("/api/applications")
        .header("origin", &app.origin)
        .header("content-type", "application/json")
        .header("content-length", data.len())
        .body(Body::from(data))
        .unwrap();
    let response = router(app).oneshot(request).await.unwrap();
    let status = response.status().as_u16();
    let value =
        serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();
    (status, value)
}
#[tokio::test]
#[ignore = "Requires an explicitly provisioned empty throwaway careers database"]
async fn postgres_submission_is_atomic_and_rate_limits_concurrent_requests() {
    let (database, _fixture) = database_fixture().await;
    let parsed = url::Url::parse(&database).unwrap();
    assert!(["/cbgpu_runtime_test", "/cbgpu_careers_test"].contains(&parsed.path()));
    assert_eq!(parsed.username(), "cbgpu_careers");
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(4)
        .connect(&database)
        .await
        .unwrap();
    let existing: i64 = sqlx::query_scalar("SELECT count(*) FROM careers.applications")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(existing, 0, "Refusing a nonempty database");
    let app = App {
        pool: pool.clone(),
        key: Arc::new(vec![b'a'; 64]),
        roles: Arc::new(json!([{"id":"frontend"}])),
        public: "unused".into(),
        origin: "https://careers.cbgpu.com".into(),
        release: "isolated-test".into(),
    };
    let token = issue_token(&app.key, chrono::Utc::now().timestamp());
    let mut jobs = tokio::task::JoinSet::new();
    for _ in 0..16 {
        jobs.spawn(submit(app.clone(), token.clone()));
    }
    let mut inserted = 0;
    let mut reference = None;
    while let Some(result) = jobs.join_next().await {
        let (status, value) = result.unwrap();
        assert!([200, 201].contains(&status), "Submission failed: {status}");
        inserted += usize::from(status == 201);
        if let Some(reference) = &reference {
            assert_eq!(reference, &value["reference"]);
        } else {
            reference = Some(value["reference"].clone());
        }
    }
    assert_eq!(inserted, 1);
    let mut jobs = tokio::task::JoinSet::new();
    for _ in 0..12 {
        jobs.spawn(submit(
            app.clone(),
            issue_token(&app.key, chrono::Utc::now().timestamp()),
        ));
    }
    let mut accepted = 0;
    let mut limited = 0;
    while let Some(result) = jobs.join_next().await {
        match result.unwrap().0 {
            201 => accepted += 1,
            429 => limited += 1,
            status => panic!("Unexpected status {status}"),
        }
    }
    assert_eq!((accepted, limited), (4, 8));
    assert_eq!(
        submit(app.clone(), token).await.0,
        200,
        "A retry must remain idempotent after the IP limit"
    );
    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM careers.applications")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(rows, 5);
    let requests: i64 = sqlx::query_scalar("SELECT count(*) FROM careers.requests")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(requests, 5);
    assert!(
        sqlx::query("SELECT * FROM operations.runs")
            .fetch_all(&pool)
            .await
            .is_err(),
        "Careers must not read operation audit data"
    );
    assert!(
        sqlx::query("CREATE TABLE careers.unexpected(id integer)")
            .execute(&pool)
            .await
            .is_err(),
        "Runtime role must not own schema"
    );
    // The import refuses populated targets, then verifies every field and
    // duplicate rate-limit history row in one transaction on an empty target.
    let temporary = tempfile::tempdir().unwrap();
    let source = temporary.path().join("legacy.sqlite");
    let sqlite = sqlx::SqlitePool::connect_with(
        sqlx::sqlite::SqliteConnectOptions::new()
            .filename(&source)
            .create_if_missing(true),
    )
    .await
    .unwrap();
    sqlx::raw_sql("CREATE TABLE applications(id TEXT,token_hash TEXT,created INTEGER,role TEXT,name TEXT,email TEXT,affiliation TEXT,motivation TEXT,experience TEXT,portfolio TEXT,consent_version TEXT,status TEXT,notified INTEGER); CREATE TABLE requests(ip TEXT,created INTEGER);").execute(&sqlite).await.unwrap();
    sqlx::query("INSERT INTO applications VALUES('CB-261116-11223344',?,1700000000,'frontend','가상 학생','synthetic@example.invalid','학교','Synthetic imported motivation','Experience','https://example.invalid','2026-11-04','contacted',1)").bind("b".repeat(64)).execute(&sqlite).await.unwrap();
    for _ in 0..2 {
        sqlx::query("INSERT INTO requests VALUES(?,1700000000)")
            .bind("c".repeat(64))
            .execute(&sqlite)
            .await
            .unwrap();
    }
    sqlite.close().await;
    assert!(cbgpu_careers::import::sqlite(&pool, &source).await.is_err());
    sqlx::query("DELETE FROM careers.applications")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM careers.requests")
        .execute(&pool)
        .await
        .unwrap();
    let result = cbgpu_careers::import::sqlite(&pool, &source).await.unwrap();
    assert!(result.verified);
    assert_eq!((result.applications, result.requests), (1, 2));
    assert!(cbgpu_careers::import::sqlite(&pool, &source).await.is_err());
    let shown =
        cbgpu_careers::import::admin(&pool, &["--show".into(), "CB-261116-11223344".into()])
            .await
            .unwrap();
    assert_eq!(shown["name"], "가상 학생");
    assert_eq!(shown["notified"], 1);
    assert_eq!(shown["status"], "contacted");
}
