//! Tenant-specific revocable streams. No account identifiers enter response headers or bodies.
use crate::{Result, fail};
use axum::{
    body::Body,
    http::{Request, Response, header},
};
use futures_util::future::BoxFuture;
use hyper_util::rt::TokioIo;
use std::{sync::Arc, time::Duration};
pub type Check = Arc<dyn Fn() -> BoxFuture<'static, bool> + Send + Sync>;
async fn revoked(check: Check, interval: Duration) {
    loop {
        tokio::time::sleep(interval).await;
        if !check().await {
            return;
        }
    }
}
pub async fn proxy(
    mut request: Request<Body>,
    port: u16,
    check: Check,
    interval: Duration,
) -> Result<Response<Body>> {
    if !check().await {
        return Err(fail("Deployment execution access unavailable", 403));
    }
    let upgrade = request.headers().contains_key(header::UPGRADE)
        && request
            .headers()
            .get(header::CONNECTION)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| {
                v.split(',')
                    .any(|s| s.trim().eq_ignore_ascii_case("upgrade"))
            });
    let downstream = upgrade.then(|| hyper::upgrade::on(&mut request));
    for name in [
        "keep-alive",
        "proxy-connection",
        "proxy-authenticate",
        "proxy-authorization",
        "te",
        "trailer",
        "transfer-encoding",
    ] {
        request.headers_mut().remove(name);
    }
    if !upgrade {
        request.headers_mut().remove(header::UPGRADE);
        request
            .headers_mut()
            .insert(header::CONNECTION, "close".parse().unwrap());
    }
    let path = request
        .uri()
        .path_and_query()
        .map(|p| p.as_str())
        .unwrap_or("/")
        .to_owned();
    *request.uri_mut() = format!("http://127.0.0.1:{port}{path}")
        .parse()
        .map_err(|_| fail("Invalid deployment request", 400))?;
    let stream = tokio::time::timeout(
        Duration::from_secs(5),
        tokio::net::TcpStream::connect(("127.0.0.1", port)),
    )
    .await
    .map_err(|_| fail("Deployment unavailable", 502))?
    .map_err(|_| fail("Deployment unavailable", 502))?;
    let (mut sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
        .await
        .map_err(|_| fail("Deployment unavailable", 502))?;
    let connection = connection.with_upgrades();
    let watcher = check.clone();
    tokio::spawn(async move {
        if upgrade {
            let _ = connection.await;
        } else {
            tokio::select! {_=connection=>{},_=revoked(watcher,interval)=>{}}
        }
    });
    let mut response = sender
        .send_request(request)
        .await
        .map_err(|_| fail("Deployment unavailable", 502))?;
    if response.status() == 101 {
        let Some(downstream) = downstream else {
            return Err(fail("Unexpected deployment upgrade", 502));
        };
        let upstream = hyper::upgrade::on(&mut response);
        tokio::spawn(async move {
            if let (Ok(down), Ok(up)) = tokio::join!(downstream, upstream) {
                let (mut down, mut up) = (TokioIo::new(down), TokioIo::new(up));
                tokio::select! {_=tokio::io::copy_bidirectional(&mut down,&mut up)=>{},_=revoked(check,interval)=>{}}
            }
        });
    }
    let (parts, body) = response.into_parts();
    Ok(Response::from_parts(parts, Body::new(body)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Router, response::IntoResponse};
    use std::sync::atomic::{AtomicBool, Ordering};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    async fn bridge(port: u16, allowed: Arc<AtomicBool>) -> (u16, tokio::task::JoinHandle<()>) {
        let app = Router::new().fallback(move |request: Request<Body>| {
            let allowed = allowed.clone();
            async move {
                let check: Check = Arc::new(move || {
                    let allowed = allowed.clone();
                    Box::pin(async move { allowed.load(Ordering::SeqCst) })
                });
                match proxy(request, port, check, Duration::from_millis(20)).await {
                    Ok(response) => response,
                    Err(error) => error.into_response(),
                }
            }
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (port, task)
    }
    #[tokio::test]
    async fn actual_upgraded_tunnel_is_denied_before_connect_and_revoked_after_upgrade() {
        let upstream = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = upstream.local_addr().unwrap().port();
        let allowed = Arc::new(AtomicBool::new(false));
        let (gateway, server) = bridge(port, allowed.clone()).await;
        assert_eq!(
            reqwest::Client::new()
                .get(format!("http://127.0.0.1:{gateway}/socket"))
                .header("connection", "upgrade")
                .header("upgrade", "websocket")
                .send()
                .await
                .unwrap()
                .status(),
            403
        );
        assert!(
            tokio::time::timeout(Duration::from_millis(30), upstream.accept())
                .await
                .is_err()
        );
        allowed.store(true, Ordering::SeqCst);
        let task = tokio::spawn(async move {
            let (mut socket, _) = upstream.accept().await.unwrap();
            let mut request = [0; 2048];
            let count = socket.read(&mut request).await.unwrap();
            assert!(count > 0);
            socket.write_all(b"HTTP/1.1 101 Switching Protocols\r\nConnection: Upgrade\r\nUpgrade: websocket\r\n\r\n").await.unwrap();
            let mut bytes = [0; 128];
            let _ = socket.read(&mut bytes).await;
        });
        let mut client = tokio::net::TcpStream::connect(("127.0.0.1", gateway))
            .await
            .unwrap();
        client.write_all(b"GET /socket HTTP/1.1\r\nHost: synthetic.example.com\r\nConnection: Upgrade\r\nUpgrade: websocket\r\n\r\n").await.unwrap();
        let mut bytes = [0; 1024];
        let n = client.read(&mut bytes).await.unwrap();
        assert!(String::from_utf8_lossy(&bytes[..n]).starts_with("HTTP/1.1 101"));
        allowed.store(false, Ordering::SeqCst);
        let n = tokio::time::timeout(Duration::from_secs(1), client.read(&mut bytes))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(n, 0);
        task.await.unwrap();
        server.abort();
    }
    #[tokio::test]
    async fn actual_http_stream_closes_on_fresh_authority_loss() {
        let upstream = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = upstream.local_addr().unwrap().port();
        let task = tokio::spawn(async move {
            let (mut socket, _) = upstream.accept().await.unwrap();
            let mut input = [0; 2048];
            let count = socket.read(&mut input).await.unwrap();
            assert!(count > 0);
            socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nfirst\r\n").await.unwrap();
            loop {
                tokio::time::sleep(Duration::from_millis(10)).await;
                if socket.write_all(b"5\r\nnext!\r\n").await.is_err() {
                    break;
                }
            }
        });
        let allowed = Arc::new(AtomicBool::new(true));
        let (gateway, server) = bridge(port, allowed.clone()).await;
        let mut response = reqwest::get(format!("http://127.0.0.1:{gateway}/stream"))
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        assert!(response.chunk().await.unwrap().is_some());
        allowed.store(false, Ordering::SeqCst);
        let outcome = tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                match response.chunk().await {
                    Ok(Some(_)) => continue,
                    other => break other,
                }
            }
        })
        .await
        .unwrap();
        assert!(!matches!(outcome, Ok(Some(_))));
        task.abort();
        server.abort();
    }
}
