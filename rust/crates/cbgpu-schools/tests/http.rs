use axum::{
    Json, Router,
    body::Body,
    http::{Request, StatusCode},
    routing::{get, post},
};
use cbgpu_schools::{Config, router};
use serde_json::{Value, json};
use tower::ServiceExt;
fn config(enabled: bool, origin: String) -> Config {
    Config {
        enabled,
        backend_url: origin,
        anon_key: "fixture-anon".into(),
        client: reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap(),
    }
}
async fn body(response: axum::response::Response) -> Value {
    serde_json::from_slice(
        &axum::body::to_bytes(response.into_body(), 100000)
            .await
            .unwrap(),
    )
    .unwrap()
}
#[tokio::test]
async fn disabled_default_and_identity_spoof_are_denied() {
    let r = router(config(false, "http://127.0.0.1:1".into()))
        .oneshot(Request::get("/v1/schools").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(r.status(), 404);
    let app = router(config(true, "http://127.0.0.1:1".into()));
    let r = app
        .clone()
        .oneshot(
            Request::post("/v1/school-applications")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(r.status(), 401);
    let r = app
        .oneshot(
            Request::post("/v1/schools/cbsh/memberships")
                .header("authorization", "bEaReR fixture")
                .body(Body::from(
                    r#"{"user_id":"someone","membership_type":"STUDENT"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(r.status(), 400);
}
#[tokio::test]
async fn public_projection_query_and_private_error_sanitization() {
    let mock=Router::new().route("/auth/v1/user",get(||async{Json(json!({"id":"trusted"}))})).route("/rest/v1/rpc/cbgpu_school_api",post(|Json(v):Json<Value>|async move{if v["action"]=="directory"{assert_eq!(v["payload"]["q"],"Science school");assert_eq!(v["payload"]["limit"],"5");(StatusCode::OK,Json(json!({"items":[],"total":0})))}else{(StatusCode::BAD_REQUEST,Json(json!({"code":"XX000","message":"postgres://private-secret@database.internal"})))}}));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, mock).await.unwrap() });
    let app = router(config(true, format!("http://{address}")));
    let r = app
        .clone()
        .oneshot(
            Request::get("/v1/schools?q=Science%20school&limit=5")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(body(r).await, json!({"items":[],"total":0}));
    let r = app
        .oneshot(
            Request::get("/v1/memberships")
                .header("authorization", "Bearer fixture")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let result = body(r).await;
    assert_eq!(result["code"], "SCHOOL_REQUEST_FAILED");
    assert!(!result.to_string().contains("secret"));
    server.abort();
}
