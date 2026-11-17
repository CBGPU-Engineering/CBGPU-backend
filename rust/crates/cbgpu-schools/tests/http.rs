use axum::{
    Json, Router,
    body::Body,
    http::{HeaderMap, Request, StatusCode},
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
    let mock=Router::new().route("/auth/v1/user",get(||async{Json(json!({"id":"trusted"}))})).route("/rest/v1/rpc/cbgpu_school_api",post(|headers:HeaderMap,Json(v):Json<Value>|async move{if v["action"]=="directory"{assert_eq!(headers["authorization"],"Bearer fixture-anon");assert_eq!(v["payload"]["q"],"Science school");assert_eq!(v["payload"]["limit"],"5");(StatusCode::OK,Json(json!({"items":[],"total":0})))}else{(StatusCode::BAD_REQUEST,Json(json!({"code":"XX000","message":"postgres://private-secret@database.internal"})))}}));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, mock).await.unwrap() });
    let app = router(config(true, format!("http://{address}")));
    let r = app
        .clone()
        .oneshot(
            Request::get("/v1/schools?q=Science%20school&limit=5")
                .header("authorization", "Bearer expired-do-not-use")
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

#[tokio::test]
async fn academic_routes_bind_school_and_report_stale_preview() {
    let mock = Router::new()
        .route(
            "/auth/v1/user",
            get(|| async { Json(json!({"id":"trusted"})) }),
        )
        .route(
            "/rest/v1/rpc/cbgpu_school_academic_api",
            post(|headers: HeaderMap, Json(v): Json<Value>| async move {
                assert_eq!(headers["authorization"], "Bearer fixture");
                assert_eq!(v["payload"]["school_id"], "cbsh");
                if v["action"] == "preview" {
                    assert_eq!(v["payload"]["school_year"], "2027");
                    (StatusCode::OK, Json(json!({"supported":true,"items":[]})))
                } else {
                    assert_eq!(v["action"], "execute");
                    assert_eq!(v["payload"]["confirmed"], true);
                    (
                        StatusCode::BAD_REQUEST,
                        Json(json!({"code":"40001","message":"ACADEMIC_PREVIEW_CHANGED"})),
                    )
                }
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, mock).await.unwrap() });
    let app = router(config(true, format!("http://{address}")));
    let response = app
        .clone()
        .oneshot(
            Request::get("/v1/admin/schools/cbsh/academic/preview?school_year=2027")
                .header("authorization", "Bearer fixture")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let response = app
        .oneshot(
            Request::post("/v1/admin/schools/cbsh/academic/execute")
                .header("authorization", "Bearer fixture")
                .body(Body::from(
                    r#"{"school_year":2027,"preview_fingerprint":"stale","confirmed":true}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), 409);
    assert_eq!(body(response).await["code"], "ACADEMIC_PREVIEW_CHANGED");
    server.abort();
}
#[tokio::test]
async fn resource_contexts_are_self_only_and_resource_target_is_path_bound() {
    let mock = Router::new()
        .route(
            "/auth/v1/user",
            get(|| async { Json(json!({"id":"trusted"})) }),
        )
        .route(
            "/rest/v1/rpc/cbgpu_school_resource_contexts",
            post(|headers: HeaderMap, Json(v): Json<Value>| async move {
                assert_eq!(headers["authorization"], "Bearer fixture");
                assert_eq!(v, json!({}));
                Json(json!({"items":[],"current_school_id":null,"server_time":"fixture"}))
            }),
        )
        .route(
            "/rest/v1/rpc/cbgpu_self_resource_access",
            post(|Json(v): Json<Value>| async move {
                assert_eq!(v, json!({"target_school":"school-b"}));
                Json(json!({"school_id":"school-b","resources":{"gpu":false}}))
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, mock).await.unwrap() });
    let app = router(config(true, format!("http://{address}")));
    let r = app
        .clone()
        .oneshot(
            Request::get("/v1/resource-contexts")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(r.status(), 401);
    let r = app
        .clone()
        .oneshot(
            Request::get("/v1/resource-contexts?user_id=foreign")
                .header("authorization", "Bearer fixture")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(body(r).await["items"], json!([]));
    let r = app
        .oneshot(
            Request::get("/v1/schools/school-b/resource-access?target_school=school-a")
                .header("authorization", "Bearer fixture")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(body(r).await["school_id"], "school-b");
    server.abort();
}

#[tokio::test]
async fn operator_review_uses_real_caller_and_route_identity() {
    let mock = Router::new()
        .route(
            "/auth/v1/user",
            get(|| async { Json(json!({"id":"trusted"})) }),
        )
        .route(
            "/rest/v1/rpc/cbgpu_school_feature_operator_api",
            post(|headers: HeaderMap, Json(v): Json<Value>| async move {
                assert_eq!(headers["authorization"], "Bearer fixture");
                if v["action"] == "list" {
                    assert_eq!(v["payload"]["school_id"], "cbsh");
                    Json(json!({"items":[]}))
                } else {
                    assert_eq!(v["action"], "review");
                    assert_eq!(v["payload"]["id"], "00000000-0000-4000-8000-000000000001");
                    assert_eq!(v["payload"]["decision"], "REVIEWING");
                    Json(json!({"request":{"revision":2,"status":"REVIEWING"}}))
                }
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move { axum::serve(listener, mock).await.unwrap() });
    let app = router(config(true, format!("http://{address}")));
    let r = app
        .clone()
        .oneshot(
            Request::get("/v1/admin/feature-requests?school_id=cbsh")
                .header("authorization", "Bearer fixture")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let r = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri("/v1/admin/feature-requests/00000000-0000-4000-8000-000000000001")
                .header("authorization", "Bearer fixture")
                .body(Body::from(
                    r#"{"revision":1,"decision":"REVIEWING","operator_message":"Reviewing"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(body(r).await["request"]["revision"], 2);
    let r = app
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri("/v1/admin/feature-requests/00000000-0000-4000-8000-000000000001")
                .header("authorization", "Bearer fixture")
                .body(Body::from(
                    r#"{"id":"other","revision":1,"decision":"REVIEWING"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(r.status(), 400);
    task.abort();
}
