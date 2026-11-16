use axum::{
    Json, Router,
    body::Body,
    extract::State,
    http::{HeaderMap, Request, StatusCode},
    routing::{get, post},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use cbgpu_contests_api::{
    Api, Result,
    auth::{Auth, Sessions},
    backend::Backend,
};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use std::{
    collections::BTreeSet,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};
use tower::ServiceExt;
const OWNER: &str = "00000000-0000-4000-8000-000000000001";
const CONTEST: &str = "60000000-0000-4000-8000-000000000001";
const KEY: &str = "private-contest-fixture-key-at-least32bytes";
fn token() -> String {
    format!("e30.{}.fixture",URL_SAFE_NO_PAD.encode(serde_json::to_vec(&json!({"sub":OWNER,"session_id":"10000000-0000-4000-8000-000000000001","role":"authenticated","exp":9999999999u64})).unwrap()))
}
struct Session {
    active: AtomicBool,
    approved: AtomicBool,
}
#[async_trait::async_trait]
impl Sessions for Session {
    async fn active(&self, session: uuid::Uuid, subject: uuid::Uuid) -> Result<Option<bool>> {
        assert_eq!(subject.to_string(), OWNER);
        assert_eq!(session.to_string(), "10000000-0000-4000-8000-000000000001");
        Ok(self
            .active
            .load(Ordering::SeqCst)
            .then(|| self.approved.load(Ordering::SeqCst)))
    }
}
#[derive(Clone)]
struct Wire {
    writes: Arc<AtomicUsize>,
    token: String,
}
async fn fixture() -> (
    Router,
    Arc<Session>,
    Arc<AtomicUsize>,
    tokio::task::JoinHandle<()>,
) {
    let writes = Arc::new(AtomicUsize::new(0));
    let state = Wire {
        writes: writes.clone(),
        token: token(),
    };
    let backend=Router::new().route("/auth/v1/user",get(|State(w):State<Wire>,h:HeaderMap|async move{assert_eq!(h["authorization"],format!("Bearer {}",w.token));Json(json!({"id":OWNER}))}))
 .route("/rest/v1/contests",get(||async{Json(json!([{"id":CONTEST,"title":"Fixture contest","registration_questions":[{"id":"reason","required":true}],"participation_mode":"team","allow_individual_entries":true}]))}))
 .route("/rest/v1/rpc/register_contest_entry",post(|State(w):State<Wire>,h:HeaderMap,Json(v):Json<Value>|async move{assert_eq!(h["authorization"],format!("Bearer {}",w.token));assert_eq!(h["apikey"],"anon-fixture");assert!(h.get("x-cbgpu-owner").is_none());assert_eq!(v.as_object().unwrap().len(),6);assert!(v.get("user_id").is_none());assert_eq!(v["p_contest_id"],CONTEST);assert_eq!(v["p_answers"],json!({"reason":"study"}));w.writes.fetch_add(1,Ordering::SeqCst);Json(json!([{"id":"registration","contest_id":CONTEST,"user_id":OWNER,"attendee_name":v["p_attendee_name"],"status":"confirmed"}]))}))
 .route("/rest/v1/event_registrations",get(||async{Json(json!([{"id":"registration","contest_id":CONTEST,"user_id":OWNER,"status":"confirmed"}]))}).patch(|State(w):State<Wire>,h:HeaderMap,Json(v):Json<Value>|async move{assert_eq!(h["authorization"],format!("Bearer {}",w.token));assert_eq!(v,json!({"status":"cancelled"}));Json(json!([{"id":"registration","contest_id":CONTEST,"user_id":OWNER,"status":"cancelled"}]))}))
 .with_state(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let handle = tokio::spawn(async move { axum::serve(listener, backend).await.unwrap() });
    let sessions = Arc::new(Session {
        active: AtomicBool::new(true),
        approved: AtomicBool::new(true),
    });
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let api = Api::new(
        Arc::new(Auth {
            origin: origin.clone(),
            anon_key: "anon-fixture".into(),
            sessions: sessions.clone(),
            client: client.clone(),
            account_v2: false,
        }),
        Arc::new(Backend {
            origin,
            anon_key: "anon-fixture".into(),
            client,
        }),
        KEY.as_bytes().to_vec(),
        BTreeSet::from(["https://cbgpu.com".into()]),
    )
    .unwrap();
    (cbgpu_contests_api::router(api), sessions, writes, handle)
}
fn request(method: &str, path: &str, value: Value) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(path)
        .header("x-cbgpu-contest-key", KEY)
        .header("authorization", format!("Bearer {}", token()))
        .header("content-type", "application/json")
        .body(if value.is_null() {
            Body::empty()
        } else {
            Body::from(value.to_string())
        })
        .unwrap()
}
#[tokio::test]
async fn actual_wire_uses_caller_jwt_rpc_and_strict_identity_free_payload() {
    let (app, _, writes, handle) = fixture().await;
    let mut responses = vec![];
    for path in [
        format!("/v1/contests/{CONTEST}/registrations"),
        "/v1/contests/register".into(),
    ] {
        let mut v = json!({"attendee_name":" Student ","answers":{"reason":"study"}});
        if path.ends_with("register") {
            v["contest_id"] = json!(CONTEST)
        }
        let response = app
            .clone()
            .oneshot(request("POST", &path, v))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);
        responses.push(response.into_body().collect().await.unwrap().to_bytes());
    }
    assert_eq!(writes.load(Ordering::SeqCst), 2);
    let v: Value = serde_json::from_slice(&responses[0]).unwrap();
    assert_eq!(v["registration"]["attendee_name"], "Student");
    for field in ["user_id", "owner", "status", "ticket_code", "created_at"] {
        let mut v = json!({"attendee_name":"Student","answers":{"reason":"study"}});
        v[field] = json!(OWNER);
        assert_eq!(
            app.clone()
                .oneshot(request(
                    "POST",
                    &format!("/v1/contests/{CONTEST}/registrations"),
                    v
                ))
                .await
                .unwrap()
                .status(),
            400
        )
    }
    assert_eq!(writes.load(Ordering::SeqCst), 2);
    handle.abort();
}
#[tokio::test]
async fn signed_out_banned_or_unapproved_sessions_cannot_register_and_ai_keys_do_not_authenticate()
{
    let (app, sessions, writes, handle) = fixture().await;
    let path = format!("/v1/contests/{CONTEST}/registrations");
    let v = json!({"attendee_name":"Student","answers":{"reason":"study"}});
    sessions.active.store(false, Ordering::SeqCst);
    assert_eq!(
        app.clone()
            .oneshot(request("POST", &path, v.clone()))
            .await
            .unwrap()
            .status(),
        401
    );
    sessions.active.store(true, Ordering::SeqCst);
    sessions.approved.store(false, Ordering::SeqCst);
    assert_eq!(
        app.clone()
            .oneshot(request("POST", &path, v))
            .await
            .unwrap()
            .status(),
        403
    );
    let mut r = request("GET", "/v1/contests", Value::Null);
    r.headers_mut().insert(
        "authorization",
        "Bearer cbgpu_ai_private_inference_key".parse().unwrap(),
    );
    assert_eq!(app.clone().oneshot(r).await.unwrap().status(), 401);
    assert_eq!(writes.load(Ordering::SeqCst), 0);
    handle.abort();
}
#[tokio::test]
async fn cors_and_origin_key_do_not_admit_cookie_or_owner_spoofing() {
    let (app, _, writes, handle) = fixture().await;
    let mut r = request("GET", "/v1/contests", Value::Null);
    r.headers_mut()
        .insert("origin", "https://evil.example".parse().unwrap());
    let response = app.clone().oneshot(r).await.unwrap();
    assert_eq!(response.status(), 403);
    assert!(
        response
            .headers()
            .get("access-control-allow-origin")
            .is_none()
    );
    let mut r = request("GET", "/v1/contests", Value::Null);
    r.headers_mut()
        .insert("origin", "https://cbgpu.com".parse().unwrap());
    let response = app.clone().oneshot(r).await.unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(
        response.headers()["access-control-allow-origin"],
        "https://cbgpu.com"
    );
    assert!(
        response
            .headers()
            .get("access-control-allow-credentials")
            .is_none()
    );
    let mut r = request("GET", "/v1/contests", Value::Null);
    r.headers_mut().remove("authorization");
    r.headers_mut()
        .insert("cookie", "sb-cbgpu-auth-token.0=ignored".parse().unwrap());
    assert_eq!(app.clone().oneshot(r).await.unwrap().status(), 401);
    let mut r = request("GET", "/v1/contests", Value::Null);
    r.headers_mut()
        .insert("x-cbgpu-owner", OWNER.parse().unwrap());
    assert_eq!(app.clone().oneshot(r).await.unwrap().status(), 400);
    let mut r = request("GET", "/v1/contests", Value::Null);
    r.headers_mut().remove("x-cbgpu-contest-key");
    assert_eq!(app.clone().oneshot(r).await.unwrap().status(), 403);
    assert_eq!(writes.load(Ordering::SeqCst), 0);
    handle.abort();
}
#[tokio::test]
async fn own_lookup_cancellation_form_and_body_bounds_are_explicit() {
    let (app, _, _, handle) = fixture().await;
    for path in [
        format!("/v1/contests/{CONTEST}"),
        format!("/v1/contests/{CONTEST}/form"),
        format!("/v1/contests/{CONTEST}/registrations/me"),
    ] {
        assert_eq!(
            app.clone()
                .oneshot(request("GET", &path, Value::Null))
                .await
                .unwrap()
                .status(),
            200
        )
    }
    let response = app
        .clone()
        .oneshot(request(
            "DELETE",
            &format!("/v1/contests/{CONTEST}/registrations/me"),
            Value::Null,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(
        app.clone()
            .oneshot(request(
                "POST",
                "/v1/contests",
                json!({"title":"not a contest creation API"})
            ))
            .await
            .unwrap()
            .status(),
        404
    );
    let response = app
        .oneshot(request(
            "POST",
            &format!("/v1/contests/{CONTEST}/registrations"),
            json!({"attendee_name":"Student","answers":{"huge":"x".repeat(300000)}}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), 413);
    handle.abort();
}

#[tokio::test]
async fn bearer_scheme_is_case_insensitive_and_subject_rate_limits_are_bounded() {
    let (app, _, writes, handle) = fixture().await;
    let mut r = request("GET", "/v1/contests", Value::Null);
    r.headers_mut().insert(
        "authorization",
        format!("bEaReR {}", token()).parse().unwrap(),
    );
    assert_eq!(app.clone().oneshot(r).await.unwrap().status(), 200);
    for _ in 0..20 {
        assert_eq!(
            app.clone()
                .oneshot(request(
                    "POST",
                    &format!("/v1/contests/{CONTEST}/registrations"),
                    json!({"attendee_name":"Student","answers":{"reason":"study"}})
                ))
                .await
                .unwrap()
                .status(),
            201
        );
    }
    assert_eq!(
        app.oneshot(request(
            "POST",
            &format!("/v1/contests/{CONTEST}/registrations"),
            json!({"attendee_name":"Student","answers":{"reason":"study"}})
        ))
        .await
        .unwrap()
        .status(),
        429
    );
    assert_eq!(writes.load(Ordering::SeqCst), 20);
    handle.abort();
}
#[test]
fn database_errors_preserve_business_status_without_private_sql_details() {
    use cbgpu_contests_api::backend::database_error;
    for (code, message, status) in [
        ("P0001", "EVENT_FULL", 409),
        ("P0001", "이미 참가 신청했어요", 409),
        ("23514", "EVENT_REGISTRATION_NOT_STARTED", 409),
        ("P0001", "EVENT_REGISTRATION_CLOSED", 409),
        ("P0001", "필수 신청 질문에 답해 주세요", 400),
        ("42501", "private SQL details", 403),
    ] {
        assert_eq!(
            database_error(400, &json!({"code":code,"message":message})).status,
            status
        );
    }
    let error = database_error(
        500,
        &json!({"code":"XX000","message":"postgres/private/path/secret"}),
    );
    assert_eq!(error.status, 503);
    assert!(!error.message.contains("private/path"));
}
