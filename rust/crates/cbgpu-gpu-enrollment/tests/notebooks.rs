use axum::{
    Json, Router,
    body::{Body, to_bytes},
    extract::Request,
    http::StatusCode,
};
use cbgpu_gpu_enrollment::{Config, hex, notebooks::protocol::*, router};
use ed25519_dalek::SigningKey;
use serde_json::json;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use tower::ServiceExt;
#[tokio::test]
async fn signed_node_binding_and_user_launch_never_trust_owner_payload() {
    let calls = Arc::new(AtomicUsize::new(0));
    let count = calls.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        axum::serve(listener,Router::new().fallback(move|r:Request|{let count=count.clone();async move{match r.uri().path(){
 "/rest/v1/rpc/school_gpu_node_identity"=>{assert_eq!(r.headers()["authorization"],"Bearer service");Json(json!({"credential_version":1,"public_key":hex(SigningKey::from_bytes(&[4;32]).verifying_key().as_bytes())}))},
 "/rest/v1/rpc/school_gpu_notebook_device"=>{assert_eq!(r.headers()["authorization"],"Bearer service");let b:serde_json::Value=serde_json::from_slice(&to_bytes(r.into_body(),24000).await.unwrap()).unwrap();assert_eq!(b["p_action"],"claim");count.fetch_add(1,Ordering::SeqCst);Json(json!({"command":null}))},
 "/auth/v1/user"=>Json(json!({"id":"11111111-1111-4111-8111-111111111111"})),
 "/rest/v1/rpc/school_gpu_notebook_api"=>{assert_eq!(r.headers()["authorization"],"Bearer user-token");let b:serde_json::Value=serde_json::from_slice(&to_bytes(r.into_body(),24000).await.unwrap()).unwrap();assert_eq!(b["p_action"],"launch");assert_eq!(b["p_school_id"],"target-school");assert_eq!(b["p_payload"]["reservation_id"],"11111111-1111-4111-8111-111111111111");assert!(b["p_payload"].get("owner_id").is_none());Json(json!({"job":{"status":"QUEUED"},"notebook_url":null}))},_=>panic!("unexpected RPC")}}})).await.unwrap()
    });
    let central = Arc::new(SigningKey::from_bytes(&[5; 32]));
    let app = router(Config {
        enabled: true,
        diagnostics: None,
        notebooks: Some(central.clone()),
        backend_url: format!("http://{addr}"),
        anon_key: "anon".into(),
        service_key: "service".into(),
        client: reqwest::Client::new(),
    });
    let path = "/v1/gpu/notebooks/claim";
    let req = NodeRequest {
        protocol: 3,
        node_id: "11111111-1111-4111-8111-111111111111".into(),
        credential_version: 1,
        sequence: 1,
        issued_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64,
        method: "POST".into(),
        path: path.into(),
        payload: Payload::Claim {},
    };
    let signed = SignedNodeRequest {
        signature: sign_node_request(&req, &SigningKey::from_bytes(&[4; 32])),
        request: req,
    };
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(path)
                .body(Body::from(serde_json::to_vec(&signed).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let envelope: SignedEnvelope =
        serde_json::from_slice(&to_bytes(response.into_body(), 24000).await.unwrap()).unwrap();
    assert!(verify_bound_envelope(
        &envelope,
        &signed.request.node_id,
        1,
        1,
        "POST",
        path,
        &hex(central.verifying_key().as_bytes())
    ));
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/gpu/notebooks/result")
                .body(Body::from(serde_json::to_vec(&signed).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let path =
        "/v1/schools/target-school/gpu/reservations/11111111-1111-4111-8111-111111111111/launch";
    for (owner, status) in [(false, StatusCode::OK), (true, StatusCode::BAD_REQUEST)] {
        let mut b = json!({"client_request_id":"22222222-2222-4222-8222-222222222222","temporary_storage_consent":true});
        if owner {
            b["owner_id"] = json!("spoof");
        }
        let r = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(path)
                    .header("authorization", "Bearer user-token")
                    .body(Body::from(b.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(r.status(), status);
    }
    task.abort();
}

#[tokio::test]
async fn identity_service_outages_are_not_permanent_node_revocation() {
    for (status, body, expected) in [
        (
            StatusCode::UNAUTHORIZED,
            json!({"code":"PGRST303","message":"JWT expired"}),
            StatusCode::SERVICE_UNAVAILABLE,
        ),
        (
            StatusCode::FORBIDDEN,
            json!({"code":"42501","message":"SERVICE_REQUIRED"}),
            StatusCode::SERVICE_UNAVAILABLE,
        ),
        (
            StatusCode::SERVICE_UNAVAILABLE,
            json!({"message":"unavailable"}),
            StatusCode::SERVICE_UNAVAILABLE,
        ),
        (
            StatusCode::FORBIDDEN,
            json!({"code":"42501","message":"NODE_UNAVAILABLE"}),
            StatusCode::FORBIDDEN,
        ),
    ] {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            axum::serve(
                listener,
                Router::new().fallback(move || {
                    let body = body.clone();
                    async move { (status, Json(body)) }
                }),
            )
            .await
            .unwrap()
        });
        let config = Config {
            enabled: true,
            diagnostics: None,
            notebooks: Some(Arc::new(SigningKey::from_bytes(&[5; 32]))),
            backend_url: format!("http://{address}"),
            anon_key: "anon".into(),
            service_key: "service".into(),
            client: reqwest::Client::new(),
        };
        let path = "/v1/gpu/notebooks/claim";
        let request = NodeRequest {
            protocol: 3,
            node_id: "11111111-1111-4111-8111-111111111111".into(),
            credential_version: 1,
            sequence: 1,
            issued_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs() as i64,
            method: "POST".into(),
            path: path.into(),
            payload: Payload::Claim {},
        };
        let signature = sign_node_request(&request, &SigningKey::from_bytes(&[4; 32]));
        let response = router(config)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(path)
                    .body(Body::from(
                        serde_json::to_vec(&SignedNodeRequest { request, signature }).unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), expected);
        task.abort();
    }
}

#[tokio::test]
async fn bootstrap_route_binds_only_job_and_forwards_caller_jwt() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        axum::serve(listener,Router::new().fallback(|r:Request|async move{assert_eq!(r.uri().path(),"/rest/v1/rpc/school_gpu_notebook_owner_session");assert_eq!(r.headers()["authorization"],"Bearer owner-token");assert_eq!(r.headers()["apikey"],"anon");let body:serde_json::Value=serde_json::from_slice(&to_bytes(r.into_body(),1024).await.unwrap()).unwrap();assert_eq!(body,json!({"p_job_id":"11111111-1111-4111-8111-111111111111"}));Json(json!({"cookie_token":"synthetic-private-grant","job_id":"11111111-1111-4111-8111-111111111111","url":"/notebooks/11111111-1111-4111-8111-111111111111/lab","expires_at":"2030-01-01T00:00:00Z"}))})).await.unwrap()
    });
    let app = router(Config {
        enabled: true,
        diagnostics: None,
        notebooks: Some(Arc::new(SigningKey::from_bytes(&[5; 32]))),
        backend_url: format!("http://{address}"),
        anon_key: "anon".into(),
        service_key: "service".into(),
        client: reqwest::Client::new(),
    });
    let path = "/v1/gpu/notebooks/11111111-1111-4111-8111-111111111111/session";
    for (auth, body, status) in [
        (None, "{}", StatusCode::UNAUTHORIZED),
        (
            Some("Bearer owner-token"),
            "{\"owner_id\":\"spoof\"}",
            StatusCode::BAD_REQUEST,
        ),
        (Some("Bearer owner-token"), "{}", StatusCode::OK),
    ] {
        let mut r = Request::builder().method("POST").uri(path);
        if let Some(auth) = auth {
            r = r.header("authorization", auth);
        }
        let response = app
            .clone()
            .oneshot(r.body(Body::from(body)).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), status);
        if status == StatusCode::OK {
            assert_eq!(response.headers()["cache-control"], "private, no-store");
        }
    }
    task.abort();
}
