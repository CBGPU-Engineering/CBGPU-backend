use axum::{
    Json, Router,
    body::Body,
    extract::State,
    http::{HeaderMap, HeaderName, Method, Request, Response, StatusCode, Uri},
    response::IntoResponse,
};
use hyper_util::{
    client::legacy::{Client, connect::HttpConnector},
    rt::{TokioExecutor, TokioIo, TokioTimer},
};
use serde_json::json;
use std::{
    net::SocketAddr,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

#[derive(Clone)]
pub struct Gateway {
    pub upstream: SocketAddr,
    pub listen: SocketAddr,
    pub release: Arc<str>,
    client: Client<HttpConnector, Body>,
    pub requests: Arc<AtomicU64>,
    pub total: Arc<AtomicU64>,
    pub upgrades: Arc<AtomicU64>,
}
impl Gateway {
    pub fn new(
        upstream: SocketAddr,
        listen: SocketAddr,
        release: &str,
    ) -> Result<Self, &'static str> {
        if !upstream.ip().is_loopback() || !listen.ip().is_loopback() || upstream == listen {
            return Err("Use distinct numeric loopback endpoints");
        }
        let mut connector = HttpConnector::new();
        connector.set_connect_timeout(Some(Duration::from_secs(5)));
        connector.set_nodelay(true);
        let client = Client::builder(TokioExecutor::new())
            .pool_timer(TokioTimer::new())
            .pool_idle_timeout(Duration::from_secs(30))
            .pool_max_idle_per_host(32)
            .build(connector);
        Ok(Self {
            upstream,
            listen,
            release: Arc::from(release),
            client,
            requests: Arc::new(AtomicU64::new(0)),
            total: Arc::new(AtomicU64::new(0)),
            upgrades: Arc::new(AtomicU64::new(0)),
        })
    }
}
struct Counter(Arc<AtomicU64>);
impl Counter {
    fn new(value: Arc<AtomicU64>) -> Self {
        value.fetch_add(1, Ordering::Relaxed);
        Self(value)
    }
}
impl Drop for Counter {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}
fn strip_hop_headers(headers: &mut HeaderMap, upgrade: bool) {
    let named: Vec<HeaderName> = headers
        .get_all("connection")
        .iter()
        .filter_map(|s| s.to_str().ok())
        .flat_map(|s| s.split(','))
        .filter_map(|s| HeaderName::from_bytes(s.trim().as_bytes()).ok())
        .collect();
    for name in named {
        if !upgrade || name != "upgrade" {
            headers.remove(name);
        }
    }
    for name in [
        "proxy-connection",
        "proxy-authenticate",
        "proxy-authorization",
        "keep-alive",
        "te",
        "trailer",
        "transfer-encoding",
    ] {
        headers.remove(name);
    }
    if upgrade {
        headers.insert("connection", "upgrade".parse().expect("constant header"));
    } else {
        headers.remove("connection");
        headers.remove("upgrade");
    }
}
fn error(status: StatusCode, message: &'static str) -> Response<Body> {
    let mut response = (status, Json(json!({"message":message}))).into_response();
    response
        .headers_mut()
        .insert("cache-control", "no-store".parse().expect("constant"));
    response
}
async fn proxy(State(state): State<Gateway>, mut request: Request<Body>) -> Response<Body> {
    // The probe is only on the private listener's own Host. Tenant /health paths
    // and all ordinary application paths always go to their original backend.
    if request.uri().path() == "/__cbgpu_gateway/health"
        && request.headers().get("host").and_then(|h| h.to_str().ok())
            == Some(&state.listen.to_string())
    {
        return Json(json!({"status":"ok","implementation":"rust-axum-gateway","built_source":option_env!("CBGPU_SOURCE_SHA"),"commit":state.release.as_ref(),"requests":state.requests.load(Ordering::Relaxed),"requests_total":state.total.load(Ordering::Relaxed),"websockets":state.upgrades.load(Ordering::Relaxed)})).into_response();
    }
    if request.method() == Method::CONNECT {
        return error(StatusCode::METHOD_NOT_ALLOWED, "CONNECT is not supported");
    }
    state.total.fetch_add(1, Ordering::Relaxed);
    let guard = Counter::new(state.requests.clone());
    let upgrade = request.headers().contains_key("upgrade")
        && request
            .headers()
            .get_all("connection")
            .iter()
            .filter_map(|v| v.to_str().ok())
            .flat_map(|v| v.split(','))
            .any(|v| v.trim().eq_ignore_ascii_case("upgrade"));
    let downstream = if upgrade {
        Some(hyper::upgrade::on(&mut request))
    } else {
        None
    };
    let path = request
        .uri()
        .path_and_query()
        .map(|p| p.as_str())
        .unwrap_or("/");
    let uri: Uri = match format!("http://{}{path}", state.upstream).parse() {
        Ok(uri) => uri,
        Err(_) => return error(StatusCode::BAD_REQUEST, "Invalid request path"),
    };
    *request.uri_mut() = uri;
    strip_hop_headers(request.headers_mut(), upgrade);
    // Preserve long-running user endpoints. Only dialing is timed out; response
    // headers/bodies and upgraded connections are not given a new migration limit.
    let mut response = match state.client.request(request).await {
        Ok(response) => response,
        Err(_) => return error(StatusCode::BAD_GATEWAY, "Upstream temporarily unavailable"),
    };
    if response.status() == StatusCode::SWITCHING_PROTOCOLS {
        let Some(downstream) = downstream else {
            return error(StatusCode::BAD_GATEWAY, "Unexpected upstream upgrade");
        };
        let upstream = hyper::upgrade::on(&mut response);
        let tunnel = Counter::new(state.upgrades.clone());
        tokio::spawn(async move {
            let _guard = guard;
            let _tunnel = tunnel;
            if let (Ok(a), Ok(b)) = tokio::join!(downstream, upstream) {
                let _ =
                    tokio::io::copy_bidirectional(&mut TokioIo::new(a), &mut TokioIo::new(b)).await;
            }
        });
        strip_hop_headers(response.headers_mut(), true);
        return response.map(Body::new);
    }
    strip_hop_headers(response.headers_mut(), false);
    // Keep the in-flight guard alive until the response body is consumed/dropped,
    // including long-running SSE/download responses. No payload is buffered/logged.
    let (parts, body) = response.into_parts();
    Response::from_parts(
        parts,
        Body::new(TrackedBody {
            body,
            guard: Some(guard),
        }),
    )
}
struct TrackedBody {
    body: hyper::body::Incoming,
    guard: Option<Counter>,
}
impl hyper::body::Body for TrackedBody {
    type Data = hyper::body::Bytes;
    type Error = hyper::Error;
    fn poll_frame(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Result<hyper::body::Frame<Self::Data>, Self::Error>>> {
        let result = std::pin::Pin::new(&mut self.body).poll_frame(cx);
        if matches!(result, std::task::Poll::Ready(None)) {
            self.guard.take();
        }
        result
    }
    fn is_end_stream(&self) -> bool {
        self.body.is_end_stream()
    }
    fn size_hint(&self) -> hyper::body::SizeHint {
        self.body.size_hint()
    }
}
pub fn router(gateway: Gateway) -> Router {
    Router::new().fallback(proxy).with_state(gateway)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn restricts_upstream() {
        assert!(
            Gateway::new(
                "192.168.1.1:3510".parse().unwrap(),
                "127.0.0.1:3511".parse().unwrap(),
                "test"
            )
            .is_err()
        );
    }
    #[test]
    fn removes_hop_by_hop_headers_without_exposing_credentials() {
        let mut headers = HeaderMap::new();
        headers.insert("connection", "keep-alive, x-remove".parse().unwrap());
        headers.insert("x-remove", "internal".parse().unwrap());
        headers.insert("authorization", "Bearer example".parse().unwrap());
        strip_hop_headers(&mut headers, false);
        assert!(!headers.contains_key("x-remove"));
        assert!(!headers.contains_key("connection"));
        assert!(headers.contains_key("authorization"));
    }
}
