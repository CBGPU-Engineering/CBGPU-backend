use axum::{
    Json, Router,
    body::{Body, to_bytes},
    extract::{Request, State},
    http::StatusCode,
    response::IntoResponse,
};
use cbgpu_gpu_enrollment::{Config, router};
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use tower::ServiceExt;
async fn backend(
    State(calls): State<Arc<AtomicUsize>>,
    request: Request,
) -> axum::response::Response {
    assert_eq!(request.headers()["apikey"], "synthetic-anon");
    assert_eq!(
        request.headers()["authorization"],
        "Bearer synthetic-caller"
    );
    if request.uri().path() == "/auth/v1/user" {
        return Json(json!({"id":"synthetic-verified"})).into_response();
    }
    assert_eq!(
        request.uri().path(),
        "/rest/v1/rpc/school_gpu_reservation_admin"
    );
    calls.fetch_add(1, Ordering::SeqCst);
    let body: Value =
        serde_json::from_slice(&to_bytes(request.into_body(), 8192).await.unwrap()).unwrap();
    assert_eq!(body["p_school_id"], "school-b");
    if body["p_action"] == "list" {
        assert_eq!(body["p_payload"]["limit"], 50);
        assert_eq!(body["p_payload"]["status"], "ended");
        return Json(json!({"school_id":"school-b","items":[],"next_cursor":null,"server_now":"2030-01-01T00:00:00Z"})).into_response();
    }
    assert_eq!(
        body["p_payload"]["reservation_id"],
        "11111111-1111-4111-8111-111111111111"
    );
    let (status, value) = match body["p_payload"]["expected_status"].as_str().unwrap() {
        "pending" => (
            StatusCode::BAD_REQUEST,
            json!({"code":"40001","message":"GPU_CURRENTLY_IN_USE"}),
        ),
        _ => (
            StatusCode::BAD_REQUEST,
            json!({"code":"XX999","message":"private backend SQL and credentials must never be forwarded"}),
        ),
    };
    (status, Json(value)).into_response()
}
#[tokio::test]
async fn scoped_calls_forward_only_caller_token_and_return_stable_failures() {
    let calls = Arc::new(AtomicUsize::new(0));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let state = calls.clone();
    let task = tokio::spawn(async move {
        axum::serve(listener, Router::new().fallback(backend).with_state(state))
            .await
            .unwrap();
    });
    let app = router(Config {
        reservations: None,
        enabled: true,
        diagnostics: None,
        notebooks: None,
        backend_url: format!("http://{address}"),
        anon_key: "synthetic-anon".into(),
        service_key: "must-not-be-forwarded".into(),
        client: reqwest::Client::new(),
    });
    for (uri, auth, status) in [
        (
            "/v1/schools/school-b/gpu/admin/reservations?limit=50&status=ended",
            true,
            StatusCode::OK,
        ),
        (
            "/v1/schools/school-b/gpu/admin/reservations?limit=50&limit=60",
            true,
            StatusCode::BAD_REQUEST,
        ),
        (
            "/v1/schools/school-b/gpu/admin/reservations?limit=50",
            false,
            StatusCode::UNAUTHORIZED,
        ),
    ] {
        let mut request = Request::get(uri);
        if auth {
            request = request.header("authorization", "Bearer synthetic-caller");
        }
        let response = app
            .clone()
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), status);
    }
    for (expected, status, code) in [
        ("pending", StatusCode::CONFLICT, "GPU_CURRENTLY_IN_USE"),
        (
            "RESERVED",
            StatusCode::SERVICE_UNAVAILABLE,
            "GPU_ADMIN_UNAVAILABLE",
        ),
    ] {
        let response=app.clone().oneshot(Request::post("/v1/schools/school-b/gpu/admin/reservations/11111111-1111-4111-8111-111111111111/action").header("authorization","Bearer synthetic-caller").body(Body::from(json!({"source":"registry","action":"revoke","expected_status":expected,"expected_end_time":"2030-01-01T00:00:00Z"}).to_string())).unwrap()).await.unwrap();
        assert_eq!(response.status(), status);
        let body: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
        assert_eq!(body, json!({"error":code}));
    }
    assert_eq!(calls.load(Ordering::SeqCst), 3);
    task.abort();
}
