use axum::{
    Json, Router,
    body::{Body, to_bytes},
    http::{Request, Response, StatusCode},
    routing::{any, get},
};
use cbgpu_gateway::{Gateway, router};
use serde_json::json;
use std::{
    sync::atomic::Ordering,
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};
async fn echo(request: Request<Body>) -> Response<Body> {
    let (parts, body) = request.into_parts();
    let body = to_bytes(body, 1024 * 1024).await.unwrap();
    let mut response=Json(json!({"path":parts.uri.to_string(),"method":parts.method.as_str(),"host":parts.headers.get("host").and_then(|s|s.to_str().ok()),"authorization":parts.headers.get("authorization").and_then(|s|s.to_str().ok()),"removed":parts.headers.contains_key("x-remove"),"body":String::from_utf8_lossy(&body)})).into_response();
    response
        .headers_mut()
        .append("set-cookie", "a=1; HttpOnly".parse().unwrap());
    response
        .headers_mut()
        .append("set-cookie", "b=2; Secure".parse().unwrap());
    response
}
use axum::response::IntoResponse;
async fn stream() -> Response<Body> {
    let stream = futures_util::stream::unfold(0, |index| async move {
        if index > 2 {
            return None;
        }
        if index > 0 {
            tokio::time::sleep(Duration::from_millis(1000)).await;
        }
        Some((
            Ok::<_, std::io::Error>(format!("data: {index}\n\n")),
            index + 1,
        ))
    });
    Response::builder()
        .header("content-type", "text/event-stream")
        .body(Body::from_stream(stream))
        .unwrap()
}
async fn websocket(mut request: Request<Body>) -> Response<Body> {
    let upgrade = hyper::upgrade::on(&mut request);
    tokio::spawn(async move {
        let stream = upgrade.await.unwrap();
        let mut stream = hyper_util::rt::TokioIo::new(stream);
        let (mut read, mut write) = tokio::io::split(&mut stream);
        let _ = tokio::io::copy(&mut read, &mut write).await;
    });
    Response::builder()
        .status(StatusCode::SWITCHING_PROTOCOLS)
        .header("connection", "Upgrade")
        .header("upgrade", "websocket")
        .header("sec-websocket-accept", "s3pPLMBiTxaQ9kYGzzhZRbK+xOo=")
        .body(Body::empty())
        .unwrap()
}
async fn fixture() -> (
    Gateway,
    tokio::task::JoinHandle<()>,
    tokio::task::JoinHandle<()>,
    tokio::sync::oneshot::Sender<()>,
) {
    let backend = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = backend.local_addr().unwrap();
    let backend = tokio::spawn(async move {
        axum::serve(
            backend,
            Router::new()
                .route("/events", get(stream))
                .route("/ws", get(websocket))
                .fallback(any(echo)),
        )
        .await
        .unwrap()
    });
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let state = Gateway::new(address, listener.local_addr().unwrap(), "test").unwrap();
    let gateway = state.clone();
    let (tx, rx) = tokio::sync::oneshot::channel();
    let front = tokio::spawn(async move {
        axum::serve(listener, router(gateway))
            .with_graceful_shutdown(async {
                let _ = rx.await;
            })
            .await
            .unwrap()
    });
    (state, backend, front, tx)
}
#[tokio::test]
async fn preserves_paths_requests_auth_cookies_and_tenant_health() {
    let (state, backend, front, stop) = fixture().await;
    let client = reqwest::Client::new();
    let response = client
        .post(format!("http://{}/echo?q=1", state.listen))
        .header("host", "tenant.front.cbgpu.com")
        .header("authorization", "Bearer test-credential")
        .header("connection", "x-remove")
        .header("x-remove", "hop")
        .body("hello")
        .send()
        .await
        .unwrap();
    assert_eq!(response.headers().get_all("set-cookie").iter().count(), 2);
    let data: serde_json::Value = response.json().await.unwrap();
    assert_eq!(data["path"], "/echo?q=1");
    assert_eq!(data["host"], "tenant.front.cbgpu.com");
    assert_eq!(data["authorization"], "Bearer test-credential");
    assert_eq!(data["body"], "hello");
    assert_eq!(data["removed"], false);
    let response = client
        .get(format!("http://{}/__cbgpu_gateway/health", state.listen))
        .header("host", "tenant.front.cbgpu.com")
        .send()
        .await
        .unwrap();
    let data: serde_json::Value = response.json().await.unwrap();
    assert!(data.get("implementation").is_none());
    let _ = stop.send(());
    front.abort();
    backend.abort();
}
#[tokio::test]
async fn sse_streams_immediately_and_is_counted_until_finished() {
    let (state, backend, front, stop) = fixture().await;
    let start = Instant::now();
    let mut response = reqwest::get(format!("http://{}/events", state.listen))
        .await
        .unwrap();
    let first = response.chunk().await.unwrap().unwrap();
    assert!(String::from_utf8_lossy(&first).contains("data: 0"));
    assert!(
        start.elapsed() < Duration::from_millis(900),
        "Do not buffer the completed response"
    );
    assert_eq!(state.requests.load(Ordering::Relaxed), 1);
    let rest = response.text().await.unwrap();
    assert!(rest.contains("data: 2"));
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert_eq!(state.requests.load(Ordering::Relaxed), 0);
    let _ = stop.send(());
    front.abort();
    backend.abort();
}
#[tokio::test]
async fn websocket_bytes_survive_gateway_listener_drain() {
    let (state, backend, front, stop) = fixture().await;
    let mut client = TcpStream::connect(state.listen).await.unwrap();
    client.write_all(b"GET /ws HTTP/1.1\r\nHost: tenant.server.cbgpu.com\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n").await.unwrap();
    let mut headers = Vec::new();
    loop {
        let byte = client.read_u8().await.unwrap();
        headers.push(byte);
        if headers.ends_with(b"\r\n\r\n") {
            break;
        }
        assert!(headers.len() < 8192);
    }
    assert!(String::from_utf8_lossy(&headers).starts_with("HTTP/1.1 101"));
    client.write_all(b"ping").await.unwrap();
    let mut reply = [0; 4];
    client.read_exact(&mut reply).await.unwrap();
    assert_eq!(&reply, b"ping");
    assert_eq!(state.upgrades.load(Ordering::Relaxed), 1);
    stop.send(()).unwrap();
    tokio::time::sleep(Duration::from_millis(20)).await;
    client.write_all(b"pong").await.unwrap();
    client.read_exact(&mut reply).await.unwrap();
    assert_eq!(
        &reply, b"pong",
        "An existing user tunnel must survive listener shutdown"
    );
    drop(client);
    for _ in 0..20 {
        if state.upgrades.load(Ordering::Relaxed) == 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(state.upgrades.load(Ordering::Relaxed), 0);
    front.abort();
    backend.abort();
}
