use axum::{
    Json, Router,
    body::Body,
    http::{HeaderMap, Request, StatusCode},
    routing::{get, post},
};
use cbgpu_accounts::{Config, router};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;
fn config(url: String) -> Config {
    Config {
        backend_url: url,
        anon_key: "synthetic-public-key".into(),
        client: reqwest::Client::new(),
    }
}
#[tokio::test]
async fn unauthenticated_request_never_reaches_backend() {
    let app = router(config("http://127.0.0.1:1".into()));
    let r = app
        .oneshot(
            Request::builder()
                .uri("/v1/account/self")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
}
#[tokio::test]
async fn current_caller_token_reaches_auth_and_database_without_service_role() {
    async fn user(h: HeaderMap) -> Json<Value> {
        assert_eq!(h["authorization"], "Bearer synthetic-token");
        assert_eq!(h["apikey"], "synthetic-public-key");
        Json(json!({"id":"synthetic-user"}))
    }
    async fn rpc(h: HeaderMap, Json(v): Json<Value>) -> Json<Value> {
        assert_eq!(h["authorization"], "Bearer synthetic-token");
        assert_eq!(v["action"], "self");
        Json(json!({"account":{"account_type":"EXTERNAL"},"resources":[]}))
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new()
                .route("/auth/v1/user", get(user))
                .route("/rest/v1/rpc/cbgpu_account_api", post(rpc)),
        )
        .await
        .unwrap();
    });
    let r = router(config(format!("http://{addr}")))
        .oneshot(
            Request::builder()
                .uri("/v1/account/self")
                .header("authorization", "Bearer synthetic-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    let value: Value =
        serde_json::from_slice(&r.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(value["account"]["account_type"], "EXTERNAL");
    server.abort();
}
#[tokio::test]
async fn expired_auth_denies_even_if_database_would_allow() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new().route("/auth/v1/user", get(|| async { StatusCode::UNAUTHORIZED })),
        )
        .await
        .unwrap();
    });
    let r = router(config(format!("http://{addr}")))
        .oneshot(
            Request::builder()
                .uri("/v1/account/self")
                .header("authorization", "Bearer revoked")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
    server.abort();
}
