use axum::{Json, Router, body::Body, extract::State, http::Request};
use cbgpu_ad_revenue::{Config, router};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use tower::ServiceExt;
#[tokio::test]
async fn fresh_admin_gate_precedes_body_and_errors_never_cache_or_leak() {
    let seen = Arc::new(Mutex::new(Vec::<String>::new()));
    let calls = seen.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        axum::serve(listener,Router::new().fallback(|State(calls):State<Arc<Mutex<Vec<String>>>>,req:axum::extract::Request|async move{let good=req.headers().get("authorization").is_some_and(|v|v.to_str().is_ok_and(|v| v.eq_ignore_ascii_case("Bearer admin")));let bytes=axum::body::to_bytes(req.into_body(),1024*1024).await.unwrap();let v:Value=serde_json::from_slice(&bytes).unwrap();calls.lock().unwrap().push(v["action"].as_str().unwrap().into());if !good{(axum::http::StatusCode::FORBIDDEN,Json(json!({"private":"do not expose raw SQL detail"})))}else{(axum::http::StatusCode::OK,Json(json!({"ok":true,"kind":"ESTIMATED","devices":[]})))}}).with_state(calls)).await.unwrap()
    });
    let app = router(Config {
        enabled: true,
        origin: format!("http://{addr}"),
        anon: "public-anon".into(),
        client: reqwest::Client::new(),
    });
    let req = Request::builder()
        .method("POST")
        .uri("/v1/admin/ad-revenue/imports")
        .header("authorization", "Bearer student")
        .body(Body::from("not JSON; must not parse before authority"))
        .unwrap();
    let r = app.clone().oneshot(req).await.unwrap();
    assert_eq!(r.status(), 403);
    assert_eq!(r.headers()["cache-control"], "private, no-store");
    let b = r.into_body().collect().await.unwrap().to_bytes();
    assert!(!String::from_utf8_lossy(&b).contains("raw SQL"));
    assert_eq!(*seen.lock().unwrap(), ["authorize"]);
    let req = Request::builder()
        .uri("/v1/admin/ad-revenue/analytics?from=2026-10-01&to=2026-10-31")
        .header("authorization", "bEaReR admin")
        .body(Body::empty())
        .unwrap();
    let r = app.clone().oneshot(req).await.unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(r.headers()["cache-control"], "private, no-store");
    assert_eq!(seen.lock().unwrap().last().unwrap(), "analytics");
    let req = Request::builder()
        .uri("/v1/admin/ad-revenueimports")
        .header("authorization", "Bearer admin")
        .body(Body::empty())
        .unwrap();
    assert_eq!(app.clone().oneshot(req).await.unwrap().status(), 404);
    task.abort();
}

#[tokio::test]
async fn preview_and_confirmed_import_forward_authoritative_normalized_payload() {
    let seen = Arc::new(Mutex::new(Vec::<Value>::new()));
    let calls = seen.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        axum::serve(listener, Router::new().fallback(|State(calls): State<Arc<Mutex<Vec<Value>>>>, req: axum::extract::Request| async move {
            let bytes = axum::body::to_bytes(req.into_body(), 3*1024*1024).await.unwrap();
            let v: Value = serde_json::from_slice(&bytes).unwrap();
            calls.lock().unwrap().push(v.clone());
            Json(if v["action"] == "preview" { json!({"preview_hash":v["payload"]["preview_hash"]}) } else { json!({"ok":true}) })
        }).with_state(calls)).await.unwrap();
    });
    let app = router(Config {
        enabled: true,
        origin: format!("http://{addr}"),
        anon: "public-anon".into(),
        client: reqwest::Client::new(),
    });
    let mut input = json!({"csv":"unit,money\nDAN-vpZNfcj3tj0jMkhV,12.34\n","granularity":"PERIOD","period_start":"2026-10-01","period_end":"2026-10-31","currency":"KRW","kind":"ESTIMATED","timezone":"Asia/Seoul","columns":{"unit_id":"unit","revenue":"money"},"format":{"delimiter":",","date_format":"YYYY-MM-DD","decimal_separator":".","group_separator":""}});
    let request = |path: &str, body: &Value| {
        Request::builder()
            .method("POST")
            .uri(path)
            .header("authorization", "Bearer admin")
            .body(Body::from(serde_json::to_vec(body).unwrap()))
            .unwrap()
    };
    let r = app
        .clone()
        .oneshot(request("/v1/admin/ad-revenue/preview", &input))
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let result: Value =
        serde_json::from_slice(&r.into_body().collect().await.unwrap().to_bytes()).unwrap();
    input["preview_hash"] = result["preview_hash"].clone();
    assert_eq!(
        app.clone()
            .oneshot(request("/v1/admin/ad-revenue/imports", &input))
            .await
            .unwrap()
            .status(),
        409
    );
    assert!(!seen.lock().unwrap().iter().any(|v| v["action"] == "import"));
    input["confirmed"] = json!(true);
    assert_eq!(
        app.oneshot(request("/v1/admin/ad-revenue/imports", &input))
            .await
            .unwrap()
            .status(),
        200
    );
    let seen = seen.lock().unwrap();
    let preview = &seen.iter().find(|v| v["action"] == "preview").unwrap()["payload"];
    let imported = &seen.iter().find(|v| v["action"] == "import").unwrap()["payload"];
    assert_eq!(preview["confirmed"], false);
    assert_eq!(imported["confirmed"], true);
    assert_eq!(imported["preview_hash"], input["preview_hash"]);
    assert_eq!(imported["rows"][0]["revenue_micros"], "12340000");
    assert!(imported.get("csv").is_none());
    assert!(imported.get("owner_id").is_none());
    task.abort();
}
