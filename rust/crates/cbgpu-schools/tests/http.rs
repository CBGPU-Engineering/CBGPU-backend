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
        central_cpu_enabled: false,
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

#[tokio::test]
async fn catalog_selection_is_forwarded_to_authoritative_draft_creation() {
    let mock = Router::new()
        .route(
            "/auth/v1/user",
            get(|| async { Json(json!({"id":"trusted"})) }),
        )
        .route(
            "/rest/v1/rpc/cbgpu_school_catalog_application_api",
            post(|headers: HeaderMap, Json(v): Json<Value>| async move {
                assert_eq!(headers["authorization"], "Bearer fixture");
                assert_eq!(v["action"], "draft_create");
                assert_eq!(v["payload"]["catalog_school_id"], "official-1192068");
                Json(json!({"application":{"catalog_school_id":"official-1192068","revision":1}}))
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move { axum::serve(listener, mock).await.unwrap() });
    let r = router(config(true, format!("http://{address}")))
        .oneshot(
            Request::post("/v1/school-applications")
                .header("authorization", "Bearer fixture")
                .body(Body::from(r#"{"catalog_school_id":"official-1192068"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(
        body(r).await["application"]["catalog_school_id"],
        "official-1192068"
    );
    task.abort();
}

#[tokio::test]
async fn direct_messaging_and_mute_routes_use_caller_jwt_and_bound_thread() {
    let mock = Router::new()
        .route(
            "/auth/v1/user",
            get(|| async { Json(json!({"id":"actual-caller"})) }),
        )
        .route(
            "/rest/v1/rpc/cbgpu_school_direct_messaging_api",
            post(|headers: HeaderMap, Json(v): Json<Value>| async move {
                assert_eq!(headers["authorization"], "Bearer fixture");
                assert!(v["payload"].get("user_id").is_none());
                match v["action"].as_str().unwrap() {
                    "preferences" => Json(json!({"allow_direct_messages":false,"display_name":""})),
                    "recipients" => {
                        assert_eq!(v["payload"]["scope"], "SCHOOL");
                        assert_eq!(v["payload"]["school_id"], "school-b");
                        Json(json!({"items":[]}))
                    }
                    "direct_send" => {
                        assert_eq!(
                            v["payload"]["conversation_id"],
                            "00000000-0000-4000-8000-000000000001"
                        );
                        assert_eq!(
                            v["payload"]["client_message_id"],
                            "00000000-0000-4000-8000-000000000002"
                        );
                        Json(json!({"message":{"id":"sent"},"duplicate":false}))
                    }
                    other => panic!("unexpected action {other}"),
                }
            }),
        )
        .route(
            "/rest/v1/rpc/cbgpu_school_messenger_api",
            post(|Json(v): Json<Value>| async move {
                assert_eq!(v["action"], "message_mutes");
                Json(json!({"items":[]}))
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move { axum::serve(listener, mock).await.unwrap() });
    let app = router(config(true, format!("http://{address}")));
    for path in [
        "/v1/messaging/preferences",
        "/v1/direct-recipients?scope=SCHOOL&school_id=school-b",
        "/v1/message-mutes",
    ] {
        assert_eq!(
            app.clone()
                .oneshot(
                    Request::get(path)
                        .header("authorization", "Bearer fixture")
                        .body(Body::empty())
                        .unwrap()
                )
                .await
                .unwrap()
                .status(),
            200
        );
    }
    let r=app.clone().oneshot(Request::post("/v1/direct-conversations/00000000-0000-4000-8000-000000000001/messages").header("authorization","Bearer fixture").body(Body::from(r#"{"body":"Message","client_message_id":"00000000-0000-4000-8000-000000000002"}"#)).unwrap()).await.unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(body(r).await["message"]["id"], "sent");
    let r = app
        .oneshot(
            Request::post("/v1/direct-conversations/00000000-0000-4000-8000-000000000001/messages")
                .header("authorization", "Bearer fixture")
                .body(Body::from(
                    r#"{"conversation_id":"other","user_id":"spoof"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(r.status(), 400);
    task.abort();
}

#[tokio::test]
async fn affiliation_routes_delegate_fresh_caller_with_bound_school() {
    let mock = Router::new()
        .route(
            "/auth/v1/user",
            get(|| async { Json(json!({"id":"trusted"})) }),
        )
        .route(
            "/rest/v1/rpc/cbgpu_school_affiliation_api",
            post(|h: HeaderMap, Json(v): Json<Value>| async move {
                assert_eq!(h["authorization"], "Bearer fixture");
                assert_eq!(v["action"], "create");
                assert_eq!(v["payload"]["school_id"], "school-b");
                assert_eq!(v["payload"]["membership_type"], "TEACHER");
                Json(json!({"request":{"status":"PENDING"}}))
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, mock).await.unwrap() });
    let app = router(config(true, format!("http://{address}")));
    let r = app
        .clone()
        .oneshot(
            Request::post("/v1/schools/school-b/affiliation-requests")
                .header("authorization", "Bearer fixture")
                .body(Body::from(
                    r#"{"membership_type":"TEACHER","consent":true}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    assert_eq!(body(r).await["request"]["status"], "PENDING");
    let r = app
        .oneshot(
            Request::post("/v1/schools/school-b/affiliation-requests")
                .header("authorization", "Bearer fixture")
                .body(Body::from(r#"{"school_roles":["SCHOOL_OWNER"]}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::BAD_REQUEST);
    server.abort();
}

#[tokio::test]
async fn central_cpu_default_off_and_bound_platform_routes() {
    let denied = router(config(true, "http://127.0.0.1:1".into()))
        .oneshot(
            Request::get("/v1/central-cpu-access?purpose=compose")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(denied.status(), StatusCode::NOT_FOUND);
    let mock = Router::new()
        .route("/auth/v1/user", get(|| async { Json(json!({"id":"trusted"})) }))
        .route("/rest/v1/rpc/cbgpu_platform_cpu_api", post(|h: HeaderMap, Json(v): Json<Value>| async move {
            assert_eq!(h["authorization"], "Bearer fixture");
            if v["action"] == "self" {
                assert_eq!(v["payload"], json!({"purpose":"compose"}));
                Json(json!({"school_id":"saved-school","allowed":false,"reason":"NO_ALLOCATION"}))
            } else {
                assert_eq!(v["action"], "set");
                assert_eq!(v["payload"]["school_id"], "school-b");
                assert_eq!(v["payload"]["status"], "APPROVED");
                assert_eq!(v["payload"]["confirmed"], true);
                Json(json!({"school_id":"school-b","revision":1,"status":"APPROVED"}))
            }
        }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, mock).await.unwrap() });
    let mut cfg = config(true, format!("http://{address}"));
    cfg.central_cpu_enabled = true;
    let app = router(cfg);
    let noauth = app
        .clone()
        .oneshot(
            Request::get("/v1/central-cpu-access?purpose=compose")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(noauth.status(), StatusCode::UNAUTHORIZED);
    let own = app
        .clone()
        .oneshot(
            Request::get("/v1/central-cpu-access?purpose=compose&school_id=untrusted")
                .header("authorization", "Bearer fixture")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(own.status(), StatusCode::OK);
    assert_eq!(body(own).await["school_id"], "saved-school");
    let set = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri("/v1/admin/schools/school-b/central-cpu")
                .header("authorization", "Bearer fixture")
                .body(Body::from(
                    r#"{"revision":0,"status":"APPROVED","confirmed":true}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(set.status(), StatusCode::OK);
    let spoof = app
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri("/v1/admin/schools/school-b/central-cpu")
                .header("authorization", "Bearer fixture")
                .body(Body::from(
                    r#"{"school_id":"school-a","owner_id":"foreign"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(spoof.status(), StatusCode::BAD_REQUEST);
    server.abort();
}

#[tokio::test]
async fn community_replies_bind_parent_and_keep_scope_server_owned() {
    let mock = Router::new()
        .route(
            "/auth/v1/user",
            get(|| async { Json(json!({"id":"caller"})) }),
        )
        .route(
            "/rest/v1/rpc/cbgpu_school_messenger_api",
            post(|headers: HeaderMap, Json(v): Json<Value>| async move {
                assert_eq!(headers["authorization"], "Bearer fixture");
                assert_eq!(v["payload"]["id"], "00000000-0000-4000-8000-000000000001");
                match v["action"].as_str().unwrap() {
                    "message" => {
                        Json(json!({"message":{"id":"post","parent_id":null,"reply_count":1}}))
                    }
                    "replies" => {
                        assert_eq!(v["payload"]["limit"], "50");
                        Json(json!({"items":[],"total":0}))
                    }
                    "reply_create" => {
                        assert!(v["payload"].get("school_id").is_none());
                        assert_eq!(v["payload"]["body"], "Real reply");
                        Json(json!({"message":{"id":"reply","parent_id":"post"},"duplicate":false}))
                    }
                    other => panic!("unexpected {other}"),
                }
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move { axum::serve(listener, mock).await.unwrap() });
    let app = router(config(true, format!("http://{address}")));
    for suffix in ["", "/replies?limit=50&offset=0"] {
        let r = app
            .clone()
            .oneshot(
                Request::get(format!(
                    "/v1/messages/00000000-0000-4000-8000-000000000001{suffix}"
                ))
                .header("authorization", "Bearer fixture")
                .body(Body::empty())
                .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(r.status(), 200);
    }
    let path = "/v1/messages/00000000-0000-4000-8000-000000000001/replies";
    let r=app.clone().oneshot(Request::post(path).header("authorization","Bearer fixture").body(Body::from(r#"{"body":"Real reply","client_message_id":"00000000-0000-4000-8000-000000000002"}"#)).unwrap()).await.unwrap();
    assert_eq!(r.status(), 200);
    let r = app
        .clone()
        .oneshot(
            Request::post(path)
                .header("authorization", "Bearer fixture")
                .body(Body::from(r#"{"id":"spoof","body":"Attempt"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(r.status(), 400);
    let r = app
        .oneshot(Request::get(path).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(r.status(), 401);
    task.abort();
}

#[tokio::test]
async fn platform_school_inventory_is_private_and_forwards_only_caller_authority() {
    let mock = Router::new()
        .route(
            "/auth/v1/user",
            get(|headers: HeaderMap| async move {
                assert!(
                    headers["authorization"] == "Bearer platform"
                        || headers["authorization"] == "Bearer school-owner"
                );
                Json(json!({"id":"fixture"}))
            }),
        )
        .route(
            "/rest/v1/rpc/cbgpu_platform_school_directory",
            post(|headers: HeaderMap, Json(v): Json<Value>| async move {
                assert_eq!(headers["apikey"], "fixture-anon");
                assert!(v.get("action").is_none());
                if headers["authorization"] == "Bearer school-owner" {
                    return (
                        StatusCode::FORBIDDEN,
                        Json(json!({"code":"42501","message":"PLATFORM_ADMIN_REQUIRED"})),
                    );
                }
                assert_eq!(
                    v["payload"],
                    json!({"q":"학교","status":"SUSPENDED","limit":"10","offset":"20"})
                );
                (
                    StatusCode::OK,
                    Json(
                        json!({"items":[{"id":"suspended-school","status":"SUSPENDED"}],"total":1}),
                    ),
                )
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, mock).await.unwrap() });
    let app = router(config(true, format!("http://{address}")));
    let path = "/v1/admin/schools?q=%ED%95%99%EA%B5%90&status=SUSPENDED&limit=10&offset=20";
    let missing = app
        .clone()
        .oneshot(Request::get(path).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(missing.status(), 401);
    let denied = app
        .clone()
        .oneshot(
            Request::get(path)
                .header("authorization", "Bearer school-owner")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(denied.status(), 403);
    let allowed = app
        .clone()
        .oneshot(
            Request::get(path)
                .header("authorization", "Bearer platform")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(allowed.status(), 200);
    assert_eq!(body(allowed).await["items"][0]["status"], "SUSPENDED");
    let mutation = app
        .oneshot(
            Request::post("/v1/admin/schools")
                .header("authorization", "Bearer platform")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(mutation.status(), 404);
    server.abort();
}

#[tokio::test]
async fn school_member_empty_filters_mean_all_and_nonempty_filters_are_preserved() {
    let mock = Router::new()
        .route(
            "/auth/v1/user",
            get(|| async { Json(json!({"id":"caller"})) }),
        )
        .route(
            "/rest/v1/rpc/cbgpu_school_api",
            post(|headers: HeaderMap, Json(v): Json<Value>| async move {
                assert_eq!(headers["authorization"], "Bearer fixture");
                assert_eq!(v["action"], "members");
                assert_eq!(v["payload"]["school_id"], "school-b");
                if let Some(q) = v["payload"].get("q") {
                    assert_eq!(q, "Kim");
                    assert_eq!(v["payload"]["status"], "ACTIVE");
                } else {
                    assert!(v["payload"].get("status").is_none());
                }
                Json(json!({"items":[],"total":0}))
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move { axum::serve(listener, mock).await.unwrap() });
    let app = router(config(true, format!("http://{address}")));
    for query in ["?q=&status=", "?q=Kim&status=ACTIVE", ""] {
        let response = app
            .clone()
            .oneshot(
                Request::get(format!("/v1/schools/school-b/members{query}"))
                    .header("authorization", "Bearer fixture")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
    }
    let response = app
        .oneshot(
            Request::get("/v1/schools/school-b/members?q=&status=")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), 401);
    task.abort();
}
#[tokio::test]
async fn clubs_routes_bind_tenant_ids_and_never_accept_actor_or_window_authority() {
    let backend=Router::new()
        .route("/auth/v1/user",get(||async{Json(json!({"id":"fixture"}))}))
        .route("/rest/v1/rpc/cbgpu_school_clubs_api",post(|h:HeaderMap,Json(v):Json<Value>|async move {
            assert_eq!(h["authorization"],"Bearer club-fixture");assert_eq!(h["apikey"],"fixture-anon");
            assert_eq!(v["payload"]["school_id"],"school-a");assert!(v["payload"].get("user_id").is_none());
            Json(json!({"action":v["action"],"payload":v["payload"],"server_now":"2026-11-24T00:00:00Z"}))
        }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let handle = tokio::spawn(async move { axum::serve(listener, backend).await.unwrap() });
    let app = router(config(true, origin));
    for (method, path, input, action) in [
        (
            "GET",
            "/v1/schools/school-a/clubs?limit=2&offset=1",
            json!({}),
            "list",
        ),
        (
            "POST",
            "/v1/schools/school-a/clubs",
            json!({"campaign_id":"60000000-0000-4000-8000-000000000001","title":"Draft"}),
            "create",
        ),
        (
            "POST",
            "/v1/schools/school-a/clubs/campaigns",
            json!({"confirmed":true}),
            "campaign_create",
        ),
        (
            "PATCH",
            "/v1/schools/school-a/clubs/campaigns/60000000-0000-4000-8000-000000000001",
            json!({"expected_revision":1,"status":"PAUSED","confirmed":true}),
            "campaign_update",
        ),
        (
            "PATCH",
            "/v1/schools/school-a/clubs/60000000-0000-4000-8000-000000000002",
            json!({"expected_revision":1,"title":"Edited"}),
            "update",
        ),
        (
            "POST",
            "/v1/schools/school-a/clubs/60000000-0000-4000-8000-000000000002/ready",
            json!({"expected_revision":1}),
            "ready",
        ),
        (
            "POST",
            "/v1/schools/school-a/clubs/60000000-0000-4000-8000-000000000002/withdraw",
            json!({"expected_revision":1}),
            "withdraw",
        ),
        (
            "GET",
            "/v1/schools/school-a/clubs/60000000-0000-4000-8000-000000000002/applications?limit=10",
            json!({}),
            "applications",
        ),
        (
            "POST",
            "/v1/schools/school-a/clubs/60000000-0000-4000-8000-000000000002/applications",
            json!({"answers":"Private fixture"}),
            "apply",
        ),
        (
            "PATCH",
            "/v1/schools/school-a/clubs/60000000-0000-4000-8000-000000000002/applications/60000000-0000-4000-8000-000000000003",
            json!({"expected_revision":1,"status":"APPROVED"}),
            "application_review",
        ),
        (
            "POST",
            "/v1/schools/school-a/clubs/60000000-0000-4000-8000-000000000002/applications/60000000-0000-4000-8000-000000000003/withdraw",
            json!({"expected_revision":1}),
            "application_withdraw",
        ),
    ] {
        let r = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(path)
                    .header("authorization", "Bearer club-fixture")
                    .body(Body::from(input.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(r.status(), 200, "{method} {path}");
        let value = body(r).await;
        assert_eq!(value["action"], action);
    }
    for path in [
        "/v1/schools/school-a/clubs?limit=0",
        "/v1/schools/school-a/clubs?limit=1&limit=2",
        "/v1/schools/school-a/clubs?school_id=other",
        "/v1/schools/school-a/clubs?campaign_id=not-a-uuid",
    ] {
        let r = app
            .clone()
            .oneshot(
                Request::get(path)
                    .header("authorization", "Bearer club-fixture")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(r.status(), 400);
    }
    for input in [
        json!({"school_id":"other"}),
        json!({"owner_id":"other"}),
        json!({"status":"READY"}),
    ] {
        let r = app
            .clone()
            .oneshot(
                Request::post("/v1/schools/school-a/clubs")
                    .header("authorization", "Bearer club-fixture")
                    .body(Body::from(input.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(r.status(), 400);
    }
    let r = app
        .oneshot(
            Request::get("/v1/schools/school-a/clubs")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(r.status(), 401);
    handle.abort();
}
