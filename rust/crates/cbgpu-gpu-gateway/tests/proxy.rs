use axum::{
    Json, Router,
    body::Body,
    extract::{
        State,
        ws::{Message, WebSocketUpgrade},
    },
    http::{HeaderMap, Request},
    response::IntoResponse,
    routing::get,
};
use cbgpu_gpu_gateway::{
    Config, Gateway,
    auth::{Authority, Grant, SupabaseAuthority},
    now,
};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
const OWNER: &str = "00000000-0000-4000-8000-000000000001";
struct Access {
    valid: AtomicBool,
    ends: f64,
}
#[async_trait::async_trait]
impl Authority for Access {
    async fn authorize(&self, token: &str, gpu: u8) -> cbgpu_gpu_gateway::Result<Grant> {
        if !self.valid.load(Ordering::SeqCst) || token != format!("owner-{gpu}") {
            return Err(cbgpu_gpu_gateway::Error {
                status: 403,
                message: "Not reservation owner".into(),
            });
        }
        Ok(Grant {
            owner: OWNER.into(),
            ends_at: self.ends,
        })
    }
}
#[derive(Clone)]
struct Upstream {
    seen: Arc<AtomicU64>,
    closed: Arc<AtomicU64>,
}
async fn echo(
    State(state): State<Upstream>,
    headers: HeaderMap,
    request: Request<Body>,
) -> axum::response::Response {
    state.seen.fetch_add(1, Ordering::SeqCst);
    assert!(!headers.contains_key("authorization"));
    let cookies = headers
        .get("cookie")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(!cookies.contains("sb-") && !cookies.contains("private-refresh"));
    let body = json!({"path":request.uri().path(),"cookie":cookies});
    let mut response = Json(body).into_response();
    response.headers_mut().append(
        "set-cookie",
        "sb-cbgpu-auth-token.0=poison; Domain=.cbgpu.com; Secure"
            .parse()
            .unwrap(),
    );
    response.headers_mut().append(
        "set-cookie",
        "notebook=ok; HttpOnly; Path=/".parse().unwrap(),
    );
    response
}
async fn websocket(
    State(state): State<Upstream>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> axum::response::Response {
    assert!(!headers.contains_key("authorization"));
    assert!(
        !headers
            .get("cookie")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .contains("sb-")
    );
    let closed = state.closed.clone();
    let mut response = ws
        .on_upgrade(move |mut socket| async move {
            while let Some(message) = socket.recv().await {
                match message {
                    Ok(Message::Close(_)) | Err(_) => break,
                    Ok(value) => {
                        if socket.send(value).await.is_err() {
                            break;
                        }
                    }
                }
            }
            closed.fetch_add(1, Ordering::SeqCst);
        })
        .into_response();
    response.headers_mut().append(
        "set-cookie",
        "cbgpu-root=poison; Domain=.cbgpu.com".parse().unwrap(),
    );
    response
        .headers_mut()
        .append("set-cookie", "notebook=ws; HttpOnly".parse().unwrap());
    response
}
async fn stream() -> axum::response::Response {
    let chunks = futures_util::stream::unfold(0u64, |n| async move {
        tokio::time::sleep(Duration::from_millis(10)).await;
        Some((Ok::<_, std::io::Error>(format!("data:{n}\n\n")), n + 1))
    });
    Body::from_stream(chunks).into_response()
}
struct Fixture {
    state: Arc<Gateway>,
    access: Arc<Access>,
    address: std::net::SocketAddr,
    upstream: Upstream,
    proxy: tokio::task::JoinHandle<()>,
    server: tokio::task::JoinHandle<()>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.state.drain();
        self.proxy.abort();
        self.server.abort();
    }
}
async fn fixture(seconds: f64) -> Fixture {
    let upstream = Upstream {
        seen: Arc::new(AtomicU64::new(0)),
        closed: Arc::new(AtomicU64::new(0)),
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let target = listener.local_addr().unwrap();
    let app = Router::new()
        .route("/{gpu}/ws", get(websocket))
        .route("/{gpu}/stream", get(stream))
        .fallback(echo)
        .with_state(upstream.clone());
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let access = Arc::new(Access {
        valid: AtomicBool::new(true),
        ends: now() + seconds,
    });
    let state = Gateway::new(
        Config {
            enabled: true,
            listen: address,
            recheck: Duration::from_millis(20),
            ..Config::default()
        },
        access.clone(),
    )
    .unwrap();
    for gpu in 1..=4 {
        state.set_upstream(gpu, Some(target)).await.unwrap();
    }
    let app = cbgpu_gpu_gateway::router(state.clone());
    let proxy = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    Fixture {
        state,
        access,
        address,
        upstream,
        proxy,
        server,
    }
}
fn cookie(gpu: u8) -> String {
    let value=json!({"access_token":format!("owner-{gpu}"),"refresh_token":"private-refresh","user":{"id":"untrusted-user"}}).to_string();
    format!(
        "sb-cbgpu-auth-token.0={}; notebook=own; _xsrf=xsrf",
        percent_encoding::utf8_percent_encode(&value, percent_encoding::NON_ALPHANUMERIC)
    )
}
fn ws_request(f: &Fixture, gpu: u8, credentials: bool) -> Request<()> {
    let mut r = format!("ws://{}/{gpu}/ws", f.address)
        .into_client_request()
        .unwrap();
    r.headers_mut()
        .insert("host", "gpu.cbgpu.com".parse().unwrap());
    r.headers_mut()
        .insert("origin", "https://gpu.cbgpu.com".parse().unwrap());
    if credentials {
        r.headers_mut()
            .insert("cookie", cookie(gpu).parse().unwrap());
    }
    r
}
async fn drained(f: &Fixture) {
    for _ in 0..100 {
        if f.state.requests.load(Ordering::SeqCst) == 0
            && f.state.tunnels.load(Ordering::SeqCst) == 0
            && f.state.monitors.load(Ordering::SeqCst) == 0
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("Connection/monitor guards leaked");
}
#[tokio::test]
async fn actual_cookie_owner_and_gpu_routing_guard_every_http_request() {
    let f = fixture(10.).await;
    let client = reqwest::Client::new();
    let r = client
        .get(format!("http://{}/1/lab", f.address))
        .header("host", "gpu.cbgpu.com")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 401);
    assert_eq!(f.upstream.seen.load(Ordering::SeqCst), 0);
    let r = client
        .get(format!("http://{}/2/lab", f.address))
        .header("host", "gpu.cbgpu.com")
        .header("cookie", cookie(1))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 403);
    for gpu in 1..=4 {
        let r = client
            .get(format!("http://{}/{gpu}/lab", f.address))
            .header("host", "gpu.cbgpu.com")
            .header("cookie", cookie(gpu))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 200);
        assert_eq!(r.headers().get_all("set-cookie").iter().count(), 1);
        let v: Value = r.json().await.unwrap();
        assert_eq!(v["path"], format!("/{gpu}/lab"));
        assert_eq!(v["cookie"], "notebook=own; _xsrf=xsrf");
    }
    drained(&f).await;
}
#[tokio::test]
async fn websocket_denies_missing_other_user_and_wrong_origin_then_closes_on_revocation() {
    let f = fixture(10.).await;
    assert!(
        tokio_tungstenite::connect_async(ws_request(&f, 1, false))
            .await
            .is_err()
    );
    let mut wrong = ws_request(&f, 2, true);
    wrong
        .headers_mut()
        .insert("cookie", cookie(1).parse().unwrap());
    assert!(tokio_tungstenite::connect_async(wrong).await.is_err());
    let mut wrong = ws_request(&f, 1, true);
    wrong
        .headers_mut()
        .insert("origin", "https://attacker.example".parse().unwrap());
    assert!(tokio_tungstenite::connect_async(wrong).await.is_err());
    let (mut ws, response) = tokio_tungstenite::connect_async(ws_request(&f, 1, true))
        .await
        .unwrap();
    assert_eq!(response.headers().get_all("set-cookie").iter().count(), 1);
    ws.send(tokio_tungstenite::tungstenite::Message::Text("ping".into()))
        .await
        .unwrap();
    assert_eq!(
        ws.next().await.unwrap().unwrap().into_text().unwrap(),
        "ping"
    );
    f.access.valid.store(false, Ordering::SeqCst);
    let closed = tokio::time::timeout(Duration::from_secs(1), ws.next())
        .await
        .unwrap();
    assert!(
        closed.is_none() || closed.is_some_and(|r| r.is_err() || r.is_ok_and(|m| m.is_close()))
    );
    drop(ws);
    drained(&f).await;
    for _ in 0..100 {
        if f.upstream.closed.load(Ordering::SeqCst) == 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(f.upstream.closed.load(Ordering::SeqCst), 1);
}
#[tokio::test]
async fn booking_expiry_and_drain_terminate_streams_without_touching_upstream_service() {
    let f = fixture(0.15).await;
    let response = reqwest::Client::new()
        .get(format!("http://{}/1/stream", f.address))
        .header("host", "gpu.cbgpu.com")
        .header("cookie", cookie(1))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let mut stream = response.bytes_stream();
    assert!(stream.next().await.unwrap().is_ok());
    let outcome = tokio::time::timeout(Duration::from_secs(1), async {
        while let Some(chunk) = stream.next().await {
            if chunk.is_err() {
                break;
            }
        }
    })
    .await;
    assert!(outcome.is_ok());
    drop(stream);
    drained(&f).await;
    let response = reqwest::Client::new()
        .get(format!("http://{}/1/lab", f.address))
        .header("host", "gpu.cbgpu.com")
        .header("cookie", cookie(1))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 401);
    let f = fixture(10.).await;
    let response = reqwest::Client::new()
        .get(format!("http://{}/1/stream", f.address))
        .header("host", "gpu.cbgpu.com")
        .header("cookie", cookie(1))
        .send()
        .await
        .unwrap();
    let mut stream = response.bytes_stream();
    assert!(stream.next().await.unwrap().is_ok());
    f.state.drain();
    tokio::time::timeout(Duration::from_secs(1), async {
        while let Some(v) = stream.next().await {
            if v.is_err() {
                break;
            }
        }
    })
    .await
    .unwrap();
    drop(stream);
    drained(&f).await;
}
#[tokio::test]
async fn native_authority_never_turns_admin_visibility_into_another_users_booking() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let app=Router::new().route("/auth/v1/user",get(||async{Json(json!({"id":OWNER}))})).route("/auth/v1/admin/users/{id}",get(||async{Json(json!({"id":OWNER,"banned_until":null}))})).route("/rest/v1/profiles",get(||async{Json(json!([{"id":OWNER,"is_admin":true,"is_approved":true}]))})).route("/rest/v1/reservations",get(||async{Json(json!([{"id":"booking","user_id":"00000000-0000-4000-8000-000000000002","gpu_id":1,"status":"approved","start_time":(chrono::Utc::now()-chrono::Duration::minutes(1)).to_rfc3339(),"end_time":(chrono::Utc::now()+chrono::Duration::minutes(1)).to_rfc3339()}]))}));
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let a = SupabaseAuthority {
        origin: format!("http://{address}"),
        anon_key: "synthetic-anon".into(),
        service_key: "synthetic-service".into(),
        account_v2: false,
        client: reqwest::Client::new(),
    };
    assert_eq!(
        a.authorize("verified-test-token", 1)
            .await
            .unwrap_err()
            .status,
        403
    );
    server.abort();
}

#[tokio::test]
async fn form_posts_preserve_same_origin_without_allowing_null_or_external_origins() {
    let f = fixture(10.).await;
    let client = reqwest::Client::new();
    let get = client
        .get(format!("http://{}/1/login", f.address))
        .header("host", "gpu.cbgpu.com")
        .header("cookie", cookie(1))
        .send()
        .await
        .unwrap();
    assert_eq!(get.status(), 200);
    assert_eq!(get.headers()["referrer-policy"], "strict-origin");
    for (origin, status) in [
        ("https://gpu.cbgpu.com", 200),
        ("null", 403),
        ("https://attacker.example", 403),
    ] {
        let response = client
            .post(format!("http://{}/1/login", f.address))
            .header("host", "gpu.cbgpu.com")
            .header("cookie", cookie(1))
            .header("origin", origin)
            .body("password=synthetic")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), status, "origin={origin}");
    }
    drained(&f).await;
}
