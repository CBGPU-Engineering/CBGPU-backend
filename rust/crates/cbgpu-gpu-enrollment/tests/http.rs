use axum::{
    Json, Router,
    body::Body,
    extract::{Request, State},
    http::StatusCode,
};
use cbgpu_gpu_enrollment::{Config, GpuHealth, Health, Report, hex, router};
use ed25519_dalek::{Signer, SigningKey};
use serde_json::json;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use tower::ServiceExt;
async fn mock(State(calls): State<Arc<AtomicUsize>>, request: Request) -> Json<serde_json::Value> {
    match request.uri().path() {
        "/rest/v1/rpc/school_gpu_node_identity" => {
            assert_eq!(request.headers()["apikey"], "synthetic-service");
            Json(
                json!({"public_key":hex(SigningKey::from_bytes(&[3;32]).verifying_key().as_bytes()),"credential_version":1}),
            )
        }
        "/rest/v1/rpc/school_gpu_store_report" => {
            assert_eq!(
                request.headers()["authorization"],
                "Bearer synthetic-service"
            );
            calls.fetch_add(1, Ordering::SeqCst);
            Json(json!({"ok":true,"execution_enabled":false,"sharing_enabled":false}))
        }
        _ => panic!("Unexpected backend operation"),
    }
}
#[tokio::test]
async fn only_verified_inventory_reaches_privileged_store() {
    let calls = Arc::new(AtomicUsize::new(0));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let count = calls.clone();
    let task = tokio::spawn(async move {
        axum::serve(listener, Router::new().fallback(mock).with_state(count))
            .await
            .unwrap()
    });
    let app = router(Config {
        enabled: true,
        backend_url: format!("http://{address}"),
        anon_key: "synthetic-anon".into(),
        service_key: "synthetic-service".into(),
        client: reqwest::Client::new(),
    });
    let r = Report {
        node_id: "12345678-1234-1234-1234-123456789abc".into(),
        credential_version: 1,
        sequence: 1,
        reported_at: 1000,
        health: Health {
            agent_version: "0.1.0".into(),
            os: "ubuntu".into(),
            gpus: vec![GpuHealth {
                index: 0,
                name: "Synthetic".into(),
                memory_total_mib: 100,
                memory_used_mib: 1,
                utilization_percent: 0,
            }],
        },
    };
    let key = SigningKey::from_bytes(&[3; 32]);
    let good = hex(&key.sign(&serde_json::to_vec(&r).unwrap()).to_bytes());
    for (signature, status) in [
        ("00".repeat(64), StatusCode::FORBIDDEN),
        (good, StatusCode::OK),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/gpu/report")
                    .body(Body::from(
                        json!({"report":r,"signature":signature}).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), status);
    }
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/v1/schools/cbsh/gpu/nodes")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/gpu/execute")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    task.abort();
}
