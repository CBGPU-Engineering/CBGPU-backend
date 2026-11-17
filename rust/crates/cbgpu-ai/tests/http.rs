use axum::{
    Json, Router,
    body::Body,
    http::{Request, StatusCode},
    routing::{get, post},
};
use cbgpu_ai::{Api, Authorization, router};
use cbgpu_ai_store::{Limits, Store};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use tower::ServiceExt;
const OWNER: &str = "00000000-0000-4000-8000-000000000001";
const OTHER: &str = "00000000-0000-4000-8000-000000000002";
const KEY: &str = "synthetic-internal-key-123456";
fn auth(url: String, v2: bool) -> Authorization {
    Authorization {
        backend_url: url,
        service_key: "synthetic-service-role".into(),
        account_v2: v2,
        client: reqwest::Client::new(),
    }
}
fn request(method: &str, path: &str, owner: Option<&str>, body: Body) -> Request<Body> {
    let mut b = Request::builder()
        .method(method)
        .uri(path)
        .header("x-cbgpu-ai-key", KEY);
    if let Some(owner) = owner {
        b = b.header("x-cbgpu-owner", owner);
    }
    b.body(body).unwrap()
}
fn offline() -> (tempfile::TempDir, axum::Router) {
    let temp = tempfile::tempdir().unwrap();
    let pool = sqlx::postgres::PgPoolOptions::new()
        .connect_lazy("postgresql://unused@127.0.0.1:1/never_used")
        .unwrap();
    let store = Store::new(pool, temp.path().join("state"), None, Limits::default()).unwrap();
    let api = Arc::new(Api::new(
        store,
        KEY.as_bytes().to_vec(),
        auth("http://127.0.0.1:1".into(), true),
        "no-image".into(),
    ));
    (temp, router(api))
}
async fn body(response: axum::response::Response) -> Value {
    serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap()
}
#[tokio::test]
async fn internal_key_and_owner_required_but_liveness_is_public() {
    let (_temp, app) = offline();
    let r = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    assert_eq!(body(r).await["scheduler"]["available"], false);
    let r = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/models")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
    let r = app
        .clone()
        .oneshot(request("GET", "/models", None, Body::empty()))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
    let r = app
        .oneshot(request("GET", "/models", Some(OWNER), Body::empty()))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    assert_eq!(body(r).await["data"].as_array().unwrap().len(), 6);
}
#[tokio::test]
async fn malformed_or_oversize_json_never_enqueues() {
    let (_temp, app) = offline();
    let r = app
        .clone()
        .oneshot(request("POST", "/jobs", Some(OWNER), Body::from("[]")))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::BAD_REQUEST);
    let r = app
        .oneshot(request(
            "POST",
            "/jobs",
            Some(OWNER),
            Body::from(vec![b'x'; 3 * 1024 * 1024 + 1]),
        ))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::PAYLOAD_TOO_LARGE);
}
#[tokio::test]
async fn school_oracle_rechecked_and_revocation_is_immediate() {
    let allow = Arc::new(AtomicBool::new(true));
    let signal = allow.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new().route(
                "/rest/v1/rpc/cbgpu_service_can_use_resource",
                post(move |Json(value): Json<Value>| {
                    let signal = signal.clone();
                    async move {
                        assert_eq!(value["user_id"], OWNER);
                        assert_eq!(value["resource"], "ai");
                        Json(json!(signal.load(Ordering::SeqCst)))
                    }
                }),
            ),
        )
        .await
        .unwrap();
    });
    let a = auth(format!("http://{addr}"), true);
    a.execution(OWNER).await.unwrap();
    allow.store(false, Ordering::SeqCst);
    assert_eq!(a.execution(OWNER).await.unwrap_err().status, 403);
    server.abort();
}
#[tokio::test]
async fn legacy_auth_checks_ban_even_when_profile_is_approved() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new()
                .route(
                    "/auth/v1/admin/users/{id}",
                    get(|| async {
                        Json(json!({"id":OWNER,"banned_until":"2099-01-01T00:00:00Z"}))
                    }),
                )
                .route(
                    "/rest/v1/profiles",
                    get(|| async { Json(json!([{"id":OWNER,"is_approved":true}])) }),
                ),
        )
        .await
        .unwrap();
    });
    assert_eq!(
        auth(format!("http://{addr}"), false)
            .execution(OWNER)
            .await
            .unwrap_err()
            .status,
        403
    );
    server.abort();
}
#[tokio::test]
#[ignore = "Requires disposable CBGPU_AI_TEST_DATABASE_URL ending /cbgpu_ai_api_test"]
async fn native_routes_preserve_ownership_and_gate_only_execution() {
    let url = std::env::var("CBGPU_AI_TEST_DATABASE_URL").unwrap();
    assert!(url.ends_with("/cbgpu_ai_api_test"));
    let temp = tempfile::tempdir().unwrap();
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(8)
        .connect(&url)
        .await
        .unwrap();
    let store = Store::new(
        pool,
        temp.path().join("state"),
        None,
        Limits {
            free_floor: 0,
            ..Limits::default()
        },
    )
    .unwrap();
    store.migrate().await.unwrap();
    cbgpu_robotics::migrate(&store.pool).await.unwrap();
    let allowed = Arc::new(AtomicBool::new(false));
    let signal = allowed.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let backend = tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new()
                .route("/auth/v1/admin/users/{id}",get(|axum::extract::Path(id):axum::extract::Path<String>|async move{Json(json!({"id":id}))}))
                .route("/rest/v1/profiles",get(|axum::extract::Query(q):axum::extract::Query<std::collections::HashMap<String,String>>|async move{Json(json!([{"id":q["id"].strip_prefix("eq.").unwrap(),"account_status":"ACTIVE","is_approved":false}]))}))
                .route(
                "/rest/v1/rpc/cbgpu_service_can_use_resource",
                post(move || {
                    let s = signal.clone();
                    async move { Json(json!(s.load(Ordering::SeqCst))) }
                }),
            ),
        )
        .await
        .unwrap();
    });
    let app = router(Arc::new(Api::new(
        store,
        KEY.as_bytes().to_vec(),
        auth(format!("http://{addr}"), true),
        "native-test-no-visual".into(),
    )));
    let r = app
        .clone()
        .oneshot(request(
            "POST",
            "/projects",
            Some(OWNER),
            Body::from(r#"{"title":"Owned after graduation"}"#),
        ))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::CREATED);
    let project = body(r).await["project"]["id"].as_str().unwrap().to_owned();
    let r = app
        .clone()
        .oneshot(request("GET", "/projects", Some(OTHER), Body::empty()))
        .await
        .unwrap();
    assert_eq!(body(r).await["projects"], json!([]));
    let r = app
        .clone()
        .oneshot(request(
            "POST",
            "/jobs",
            Some(OWNER),
            Body::from(r#"{"kind":"embed","input":{"input":"test"}}"#),
        ))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::FORBIDDEN);
    let r = app
        .clone()
        .oneshot(request("GET", "/jobs", Some(OWNER), Body::empty()))
        .await
        .unwrap();
    assert_eq!(body(r).await["jobs"], json!([]));
    allowed.store(true, Ordering::SeqCst);
    let r = app
        .clone()
        .oneshot(request(
            "POST",
            "/jobs",
            Some(OWNER),
            Body::from(r#"{"kind":"embed","input":{"input":"test"}}"#),
        ))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::ACCEPTED);
    let job = body(r).await["job"]["id"].as_str().unwrap().to_owned();
    let r = app
        .clone()
        .oneshot(request(
            "GET",
            &format!("/jobs/{job}"),
            Some(OTHER),
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::NOT_FOUND);
    allowed.store(false, Ordering::SeqCst);
    let r = app
        .clone()
        .oneshot(request(
            "GET",
            &format!("/jobs/{job}"),
            Some(OWNER),
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    let r = app
        .clone()
        .oneshot(request(
            "POST",
            "/uploads?name=notes.txt",
            Some(OWNER),
            Body::from("Native text evidence"),
        ))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::CREATED);
    let upload = body(r).await["id"].as_str().unwrap().to_owned();
    let denied = app
        .clone()
        .oneshot(request(
            "POST",
            &format!("/uploads/{upload}/prepare"),
            Some(OWNER),
            Body::from("{}"),
        ))
        .await
        .unwrap();
    assert_eq!(denied.status(), StatusCode::FORBIDDEN);
    let denied = app
        .clone()
        .oneshot(request(
            "POST",
            "/web/search",
            Some(OWNER),
            Body::from(r#"{"query":"must not send an external request"}"#),
        ))
        .await
        .unwrap();
    assert_eq!(denied.status(), StatusCode::FORBIDDEN);
    allowed.store(true, Ordering::SeqCst);
    let r = app
        .clone()
        .oneshot(request(
            "POST",
            &format!("/uploads/{upload}/prepare"),
            Some(OWNER),
            Body::from("{}"),
        ))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::ACCEPTED);
    let mut ready = false;
    for _ in 0..100 {
        let r = app
            .clone()
            .oneshot(request(
                "GET",
                &format!("/uploads/{upload}"),
                Some(OWNER),
                Body::empty(),
            ))
            .await
            .unwrap();
        if body(r).await["file"]["state"] == "ready" {
            ready = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert!(ready);
    allowed.store(false, Ordering::SeqCst);
    let r = app
        .clone()
        .oneshot(request(
            "GET",
            &format!("/uploads/{upload}/text"),
            Some(OWNER),
            Body::empty(),
        ))
        .await
        .unwrap();
    assert!(
        body(r).await["text"]
            .as_str()
            .unwrap()
            .contains("Native text evidence")
    );
    let r = app
        .clone()
        .oneshot(request(
            "DELETE",
            &format!("/projects/{project}"),
            Some(OTHER),
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::NOT_FOUND);
    backend.abort();
}
#[test]
fn tenant_scope_mode_is_server_driven_and_bad_flags_never_fall_back() {
    use cbgpu_ai::scoped_authorization_mode as mode;
    for value in ["1", "true"] {
        assert!(mode([Some("0".into()), Some(value.into())]).unwrap());
    }
    assert!(!mode([None, Some("false".into())]).unwrap());
    for value in ["tru", "yes", "production", "TRUE", "FALSE", " 1", "0 "] {
        assert!(mode([Some("true".into()), Some(value.into())]).is_err());
    }
}
#[tokio::test]
async fn owned_data_identity_is_fresh_without_requiring_school_resource_approval() {
    let suspended = Arc::new(AtomicBool::new(false));
    let flag = suspended.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener,Router::new().route("/auth/v1/admin/users/{id}",get(|axum::extract::Path(id):axum::extract::Path<String>|async move{Json(json!({"id":id}))})).route("/rest/v1/profiles",get(move||{let flag=flag.clone();async move{Json(json!([{"id":OWNER,"is_approved":false,"account_status":if flag.load(Ordering::SeqCst){"SUSPENDED"}else{"REVERIFICATION_REQUIRED"}}]))}}))).await.unwrap()
    });
    let a = auth(format!("http://{address}"), true);
    a.identity(OWNER).await.unwrap();
    suspended.store(true, Ordering::SeqCst);
    assert_eq!(a.identity(OWNER).await.unwrap_err().status, 403);
    server.abort();
}
