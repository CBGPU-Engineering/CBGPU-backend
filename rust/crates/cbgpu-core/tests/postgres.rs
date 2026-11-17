use axum::{Json, Router, routing::post};
use cbgpu_core::{Settings, parse_web_origin, pool, run};
use chrono::{DateTime, Utc};
use serde_json::json;
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

#[tokio::test]
#[ignore = "Requires explicitly provisioned isolated cbgpu_runtime_test database"]
async fn postgres_claims_preserve_idempotency_and_uncertain_outcomes() {
    let database_url =
        std::env::var("CBGPU_TEST_DATABASE_URL").expect("explicit test database URL");
    assert_eq!(
        url::Url::parse(&database_url).unwrap().path(),
        "/cbgpu_runtime_test",
        "Never write test rows into production"
    );
    let directory = tempfile::tempdir().unwrap();
    let key = directory.path().join("key");
    std::fs::write(&key, "test-secret-at-least-16-characters").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    let requests = Arc::new(AtomicUsize::new(0));
    let calls = requests.clone();
    let app = Router::new()
        .route(
            "/api/cron/timetable-warm",
            post(move || {
                let calls = calls.clone();
                async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                    Json(json!({"ok":true,"days":1,"cells":1,"stale":false}))
                }
            }),
        )
        .route(
            "/api/cron/study-auto",
            post(|| async { "invalid JSON; never retry this write" }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let settings = Settings {
        database_url: database_url.clone(),
        listen: address,
        web_origin: parse_web_origin(&format!("http://{address}")).unwrap(),
        cron_secret_file: key.clone(),
        timetable_secret_file: key,
        timetable_school_id: "cbsh".into(),
        admin_key_file: PathBuf::new(),
        release: "test".into(),
    };
    let db = pool(&database_url, 1).await.unwrap();
    let at = DateTime::parse_from_rfc3339("2026-11-16T01:30:00Z")
        .unwrap()
        .with_timezone(&Utc);
    let (a, b) = tokio::join!(
        run(&settings, "timetable-refresh", at, false),
        run(&settings, "timetable-refresh", at, false)
    );
    assert!(a.is_ok() && b.is_ok());
    assert_eq!(
        requests.load(Ordering::SeqCst),
        1,
        "One transport call per claimed slot"
    );
    let count:i64=sqlx::query_scalar("SELECT count(*) FROM operations.runs WHERE operation='timetable-refresh' AND status='succeeded'").fetch_one(&db).await.unwrap();
    assert_eq!(count, 1);
    let result = run(&settings, "study-auto", at, false).await;
    assert!(result.is_err());
    let status: String =
        sqlx::query_scalar("SELECT status FROM operations.runs WHERE operation='study-auto'")
            .fetch_one(&db)
            .await
            .unwrap();
    assert_eq!(status, "uncertain");
    assert_eq!(
        run(&settings, "study-auto", at, false).await.unwrap()["skipped"],
        "slot_already_claimed"
    );
    let denied = sqlx::query("DELETE FROM operations.runs")
        .execute(&db)
        .await; // Test policy is allowed to clear rows only by fixture owner, not runtime.
    assert!(denied.is_err(), "Runtime must not have DELETE permission");
    db.close().await;
    task.abort();
}
