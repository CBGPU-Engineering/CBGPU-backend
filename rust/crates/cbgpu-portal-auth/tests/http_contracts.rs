//! Fake local GoTrue transport and unavailable disposable endpoints only.
use axum::{
    Json, Router,
    body::Body,
    http::{Request, StatusCode},
    routing::{get, post},
};
use cbgpu_portal_auth::{
    authority::{GoTrue, Oracle},
    core::{Core, Roster},
    crypto::Crypto,
    store::Store,
    *,
};
use serde_json::json;
use std::sync::Arc;
use tower::ServiceExt;
fn id() -> uuid::Uuid {
    "10000000-0000-4000-8000-000000000001".parse().unwrap()
}
fn core() -> Arc<Core> {
    let pool = sqlx::postgres::PgPoolOptions::new()
        .acquire_timeout(std::time::Duration::from_millis(100))
        .connect_lazy("postgres://fixture:fixture@127.0.0.1:9/fixture")
        .unwrap();
    let (revoked, _) = tokio::sync::broadcast::channel(8);
    Arc::new(Core {
        store: Store { pool: pool.clone() },
        crypto: Crypto::new("fixture-session-secret-not-production-123456").unwrap(),
        roster: Roster::Fixed(vec![
            Member {
                id: id(),
                email: "dev@example.invalid".into(),
            },
            Member {
                id: "10000000-0000-4000-8000-000000000002".parse().unwrap(),
                email: "other@example.invalid".into(),
            },
        ]),
        oracle: Oracle::from_pool(pool),
        gotrue: GoTrue::new("http://127.0.0.1:9/", "synthetic-anon".into(), false).unwrap(),
        scoped: true,
        revoked,
    })
}
#[tokio::test]
async fn unauthenticated_session_and_removed_steps_match_existing_ui_without_reaching_database() {
    let app = cbgpu_portal_auth::http::router(core());
    for (path, method, status) in [
        ("/api/session", "GET", 200),
        ("/api/login/system", "POST", 410),
        ("/api/mfa/enroll", "POST", 410),
        ("/portal/session", "GET", 401),
        ("/api/terminal/session", "POST", 404),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(path)
                    .method(method)
                    .header(
                        "host",
                        if path == "/portal/session" {
                            "admin.cbgpu.com"
                        } else {
                            "developers.cbgpu.com"
                        },
                    )
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status().as_u16(), status);
        assert_eq!(response.headers()["cache-control"], "no-store");
        let bytes = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap();
        if path == "/api/session" {
            assert_eq!(
                serde_json::from_slice::<serde_json::Value>(&bytes).unwrap(),
                json!({"authenticated":false})
            )
        }
    }
}
#[tokio::test]
async fn equipment_hosts_and_bad_login_origins_never_inherit_developer_access() {
    let app = cbgpu_portal_auth::http::router(core());
    for (host, origin) in [
        ("print.cbgpu.com", "https://print.cbgpu.com"),
        ("laser.cbgpu.com", "https://laser.cbgpu.com"),
        ("untrusted.front.cbgpu.com", "https://developers.cbgpu.com"),
        ("developers.cbgpu.com", "https://attacker.invalid"),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/login")
                    .method("POST")
                    .header("host", host)
                    .header("origin", origin)
                    .header("content-type", "application/json")
                    .body(Body::from(
                        "{\"email\":\"dev@example.invalid\",\"password\":\"fixture-only\"}",
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN)
    }
}
#[tokio::test]
async fn cookie_monitor_closes_on_store_or_authority_failure_without_touching_any_socket() {
    let core = core();
    let (tx, mut rx) = tokio::sync::watch::channel(false);
    let task = tokio::spawn(core.monitor("a".repeat(64), tx));
    tokio::time::timeout(std::time::Duration::from_secs(2), rx.changed())
        .await
        .unwrap()
        .unwrap();
    assert!(*rx.borrow());
    task.await.unwrap();
}
#[tokio::test]
async fn public_gotrue_login_requires_verified_user_and_fresh_self_session() {
    for valid in [true, false] {
        let user = id();
        let api=Router::new().route("/auth/v1/token",post(move|headers:axum::http::HeaderMap,Json(body):Json<serde_json::Value>|async move{assert_eq!(headers["apikey"],"synthetic-anon");assert!(headers.get("cookie").is_none());assert!(headers.get("authorization").is_none());assert_eq!(body["password"],"fixture-only");Json(json!({"user":{"id":user},"access_token":"fixture-access-token","refresh_token":"fixture-refresh"}))})).route("/auth/v1/user",get(move|headers:axum::http::HeaderMap|async move{assert_eq!(headers["authorization"],"Bearer fixture-access-token");Json(json!({"id":user}))})).route("/rest/v1/rpc/cbgpu_self_session_status",post(move|headers:axum::http::HeaderMap|async move{assert_eq!(headers["authorization"],"Bearer fixture-access-token");Json(json!({"valid":valid,"approved":false}))}));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        let task = tokio::spawn(async move { axum::serve(listener, api).await.unwrap() });
        let client = GoTrue::new(&url, "synthetic-anon".into(), false).unwrap();
        let result = client.login("dev@example.invalid", "fixture-only").await;
        if valid {
            assert_eq!(result.unwrap().0, id())
        } else {
            assert_eq!(result.err().unwrap().code, "ACCOUNT_SESSION_REVOKED")
        }
        task.abort();
    }
}
