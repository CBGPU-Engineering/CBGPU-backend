//! Candidate authenticated GPU HTTP/WebSocket gateway. It never starts/stops notebooks.
pub mod auth;
use axum::{
    Router,
    body::Body,
    extract::State,
    http::{HeaderMap, HeaderName, Method, Request, Response, StatusCode, Uri},
    response::IntoResponse,
};
use http_body_util::BodyExt;
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
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::sync::{RwLock, watch};
pub fn now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}
#[derive(Debug)]
pub struct Error {
    pub status: u16,
    pub message: String,
}
impl Error {
    pub fn unavailable() -> Self {
        Self {
            status: 503,
            message: "Notebook authorization is temporarily unavailable".into(),
        }
    }
}
pub type Result<T> = std::result::Result<T, Error>;
#[derive(Clone)]
pub struct Config {
    pub enabled: bool,
    pub listen: SocketAddr,
    pub public_host: String,
    pub cookie_key: String,
    pub recheck: Duration,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            enabled: false,
            listen: "127.0.0.1:13500".parse().unwrap(),
            public_host: "gpu.cbgpu.com".into(),
            cookie_key: "sb-cbgpu-auth-token".into(),
            recheck: Duration::from_secs(10),
        }
    }
}
pub struct Gateway {
    pub config: Config,
    pub upstreams: RwLock<[Option<SocketAddr>; 4]>,
    pub authority: Arc<dyn auth::Authority>,
    client: Client<HttpConnector, Body>,
    drain: watch::Sender<bool>,
    pub requests: Arc<AtomicU64>,
    pub tunnels: Arc<AtomicU64>,
    pub monitors: Arc<AtomicU64>,
}
impl Gateway {
    pub fn new(config: Config, authority: Arc<dyn auth::Authority>) -> Result<Arc<Self>> {
        if !config.listen.ip().is_loopback() || config.public_host.is_empty() {
            return Err(Error {
                status: 500,
                message: "Gateway listener must be private loopback".into(),
            });
        }
        let mut connector = HttpConnector::new();
        connector.set_connect_timeout(Some(Duration::from_secs(5)));
        connector.set_nodelay(true);
        let client = Client::builder(TokioExecutor::new())
            .pool_timer(TokioTimer::new())
            .pool_idle_timeout(Duration::from_secs(30))
            .pool_max_idle_per_host(32)
            .build(connector);
        let (drain, _) = watch::channel(false);
        Ok(Arc::new(Self {
            config,
            upstreams: RwLock::new([None; 4]),
            authority,
            client,
            drain,
            requests: Arc::new(AtomicU64::new(0)),
            tunnels: Arc::new(AtomicU64::new(0)),
            monitors: Arc::new(AtomicU64::new(0)),
        }))
    }
    pub fn drain(&self) {
        self.drain.send_replace(true);
    }
    pub async fn set_upstream(&self, gpu: u8, address: Option<SocketAddr>) -> Result<()> {
        if !(1..=4).contains(&gpu) {
            return Err(Error {
                status: 400,
                message: "Invalid GPU number".into(),
            });
        }
        if address.is_some_and(|a| !allowed_ip(a.ip()) || a.port() == 0) {
            return Err(Error {
                status: 400,
                message: "Notebook upstream must use a private numeric address".into(),
            });
        }
        self.upstreams.write().await[gpu as usize - 1] = address;
        Ok(())
    }
    fn monitor(
        self: &Arc<Self>,
        token: String,
        gpu: u8,
        grant: auth::Grant,
    ) -> (watch::Receiver<bool>, Lease) {
        let (revoke, receiver) = watch::channel(false);
        let (stop, mut stopped) = watch::channel(false);
        let mut drain = self.drain.subscribe();
        let state = self.clone();
        let count = Counter::new(self.monitors.clone());
        tokio::spawn(async move {
            let _count = count;
            let owner = grant.owner;
            let jwt_end = auth::token_expiry(&token);
            let mut ends = jwt_end
                .map(|v| v.min(grant.ends_at))
                .unwrap_or(grant.ends_at);
            loop {
                if *stopped.borrow() {
                    break;
                }
                if *drain.borrow() || now() >= ends {
                    revoke.send_replace(true);
                    break;
                }
                let wait = state
                    .config
                    .recheck
                    .min(Duration::from_secs_f64((ends - now()).max(0.001)));
                tokio::select! {_ =stopped.changed()=>break,_ =drain.changed()=>{revoke.send_replace(true);break;},_ =tokio::time::sleep(wait)=>{}}
                if now() >= ends {
                    revoke.send_replace(true);
                    break;
                }
                let result = tokio::select! {_ =stopped.changed()=>break,_ =drain.changed()=>{revoke.send_replace(true);break;},_ =tokio::time::sleep(Duration::from_secs_f64((ends-now()).max(0.001)))=>{revoke.send_replace(true);break;},result=state.authority.authorize(&token,gpu)=>result};
                match result {
                    Ok(grant)
                        if grant.owner == owner
                            && grant.ends_at.is_finite()
                            && grant.ends_at > now() =>
                    {
                        ends = jwt_end
                            .map(|v| v.min(grant.ends_at))
                            .unwrap_or(grant.ends_at);
                    }
                    _ => {
                        revoke.send_replace(true);
                        break;
                    }
                }
            }
        });
        (
            receiver,
            Lease {
                stop,
                _request: Counter::new(self.requests.clone()),
            },
        )
    }
}
fn allowed_ip(ip: std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(ip) => ip.is_loopback() || ip.is_private(),
        std::net::IpAddr::V6(ip) => ip.is_loopback() || ip.is_unique_local(),
    }
}
struct Counter(Arc<AtomicU64>);
impl Counter {
    fn new(v: Arc<AtomicU64>) -> Self {
        v.fetch_add(1, Ordering::Relaxed);
        Self(v)
    }
}
impl Drop for Counter {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}
struct Lease {
    stop: watch::Sender<bool>,
    _request: Counter,
}
impl Drop for Lease {
    fn drop(&mut self) {
        self.stop.send_replace(true);
    }
}
fn error(status: u16, message: &str) -> Response<Body> {
    let mut r = (
        StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
        axum::Json(json!({"message":message})),
    )
        .into_response();
    secure(r.headers_mut());
    r
}
fn secure(h: &mut HeaderMap) {
    h.insert("cache-control", "no-store".parse().unwrap());
    h.insert("x-robots-tag", "noindex, nofollow".parse().unwrap());
    h.insert(
        "strict-transport-security",
        "max-age=31536000".parse().unwrap(),
    );
    h.insert("referrer-policy", "no-referrer".parse().unwrap());
    h.insert("x-content-type-options", "nosniff".parse().unwrap());
}
fn hop(headers: &mut HeaderMap, upgrade: bool) {
    let names = headers
        .get_all("connection")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .filter_map(|s| HeaderName::from_bytes(s.trim().as_bytes()).ok())
        .collect::<Vec<_>>();
    for name in names {
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
        headers.insert("connection", "upgrade".parse().unwrap());
    } else {
        headers.remove("connection");
        headers.remove("upgrade");
    }
}
pub fn strip_credentials(headers: &mut HeaderMap, cookie_key: &str) {
    if headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("Bearer "))
    {
        headers.remove("authorization");
    }
    let _ = cookie_key; // Shared CBGPU namespace is defined centrally.
    cbgpu_edge_policy::strip_request(headers);
}
fn gpu_path(path: &str) -> Option<u8> {
    let rest = path.strip_prefix('/')?;
    let first = rest.split('/').next()?;
    let n = first.parse::<u8>().ok()?;
    if first.len() == 1 && (1..=4).contains(&n) {
        Some(n)
    } else {
        None
    }
}
pub fn router(state: Arc<Gateway>) -> Router {
    Router::new().fallback(proxy).with_state(state)
}
async fn proxy(State(state): State<Arc<Gateway>>, mut request: Request<Body>) -> Response<Body> {
    if request.uri().path() == "/__cbgpu_gpu/health"
        && request.headers().get("host").and_then(|v| v.to_str().ok())
            == Some(state.config.listen.to_string().as_str())
    {
        return axum::Json(json!({"status":"ok","candidate":true,"auth_enabled":state.config.enabled,"requests":state.requests.load(Ordering::Relaxed),"websockets":state.tunnels.load(Ordering::Relaxed),"monitors":state.monitors.load(Ordering::Relaxed)})).into_response();
    }
    if !state.config.enabled {
        return error(503, "Authenticated GPU gateway candidate is disabled");
    }
    if *state.drain.borrow() {
        return error(503, "Notebook gateway is draining");
    }
    let host = request
        .headers()
        .get("host")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if host != state.config.public_host {
        return error(400, "Invalid notebook host");
    }
    if request.method() == Method::CONNECT {
        return error(405, "CONNECT is not supported");
    }
    let Some(gpu) = gpu_path(request.uri().path()) else {
        return error(404, "Use an assigned GPU notebook URL /1/ through /4/");
    };
    let upgrade = request
        .headers()
        .get("upgrade")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.eq_ignore_ascii_case("websocket"))
        && request
            .headers()
            .get_all("connection")
            .iter()
            .filter_map(|v| v.to_str().ok())
            .flat_map(|s| s.split(','))
            .any(|v| v.trim().eq_ignore_ascii_case("upgrade"));
    let origin = request
        .headers()
        .get("origin")
        .and_then(|v| v.to_str().ok());
    let expected = format!("https://{}", state.config.public_host);
    let bearer = request
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("Bearer "));
    if upgrade && origin != Some(expected.as_str())
        || ![Method::GET, Method::HEAD, Method::OPTIONS].contains(request.method())
            && (origin.is_some_and(|o| o != expected) || !bearer && origin.is_none())
    {
        return error(403, "Notebook origin is not allowed");
    }
    for (k, v) in url_query(request.uri().query().unwrap_or("")) {
        if ["access_token", "refresh_token", "cbgpu_token"].contains(&k.as_str())
            || (k == "token" && v.starts_with("eyJ"))
        {
            return error(
                400,
                "CBGPU credentials must not be included in notebook URLs",
            );
        }
    }
    let token = match auth::credential(request.headers(), &state.config.cookie_key) {
        Ok(token) => token,
        Err(e) => return error(e.status, &e.message),
    };
    let grant = match tokio::time::timeout(
        Duration::from_secs(8),
        state.authority.authorize(&token, gpu),
    )
    .await
    {
        Ok(Ok(grant)) => grant,
        Ok(Err(e)) => return error(e.status, &e.message),
        Err(_) => return error(503, "Notebook authorization timed out"),
    };
    if !grant.ends_at.is_finite()
        || grant.ends_at <= now()
        || auth::token_expiry(&token).is_some_and(|t| t <= now())
    {
        return error(401, "Notebook session or reservation expired");
    }
    let Some(upstream) = state.upstreams.read().await[gpu as usize - 1] else {
        return error(503, "Notebook is temporarily unavailable");
    };
    if request.uri().path() == format!("/{gpu}") {
        let location = format!(
            "/{gpu}/{}",
            request
                .uri()
                .query()
                .map(|q| format!("?{q}"))
                .unwrap_or_default()
        );
        let mut response = Response::new(Body::empty());
        *response.status_mut() = StatusCode::PERMANENT_REDIRECT;
        if let Ok(location) = location.parse() {
            response.headers_mut().insert("location", location);
        }
        secure(response.headers_mut());
        return response;
    }
    let (mut revoked, lease) = state.monitor(token, gpu, grant);
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
    let uri: Uri = match format!("http://{upstream}{path}").parse() {
        Ok(v) => v,
        Err(_) => return error(400, "Invalid notebook request path"),
    };
    *request.uri_mut() = uri;
    strip_credentials(request.headers_mut(), &state.config.cookie_key);
    hop(request.headers_mut(), upgrade);
    request
        .headers_mut()
        .insert("host", state.config.public_host.parse().unwrap());
    request.headers_mut().insert(
        "x-forwarded-host",
        state.config.public_host.parse().unwrap(),
    );
    request
        .headers_mut()
        .insert("x-forwarded-proto", "https".parse().unwrap());
    request.headers_mut().remove("x-forwarded-for");
    let mut response = tokio::select! {_ =revoked.changed()=>return error(403,"Notebook access expired or was revoked"),response=state.client.request(request)=>match response{Ok(v)=>v,Err(_)=>return error(502,"Notebook is starting; retry shortly")}};
    if response.status() == StatusCode::SWITCHING_PROTOCOLS {
        let Some(downstream) = downstream else {
            return error(502, "Unexpected notebook upgrade");
        };
        let upstream = hyper::upgrade::on(&mut response);
        let counter = Counter::new(state.tunnels.clone());
        tokio::spawn(async move {
            let _lease = lease;
            let _counter = counter;
            let streams = tokio::select! {_ =revoked.changed()=>return,streams=async{tokio::join!(downstream,upstream)}=>streams};
            if let (Ok(a), Ok(b)) = streams {
                let mut a = TokioIo::new(a);
                let mut b = TokioIo::new(b);
                tokio::select! {_ =revoked.changed()=>{},_ =tokio::io::copy_bidirectional(&mut a,&mut b)=>{}}
            }
        });
        hop(response.headers_mut(), true);
        cbgpu_edge_policy::filter_response(response.headers_mut(), &state.config.public_host);
        secure(response.headers_mut());
        return response.map(Body::new);
    }
    hop(response.headers_mut(), false);
    cbgpu_edge_policy::filter_response(response.headers_mut(), &state.config.public_host);
    secure(response.headers_mut());
    let (parts, body) = response.into_parts();
    let stream = futures_util::stream::unfold(
        (body, revoked, lease),
        |(mut body, mut revoked, lease)| async move {
            if *revoked.borrow() {
                return None;
            }
            let frame =
                tokio::select! {_ =revoked.changed()=>return None,frame=body.frame()=>frame};
            match frame {
                Some(Ok(frame)) => match frame.into_data() {
                    Ok(data) => Some((Ok::<_, std::io::Error>(data), (body, revoked, lease))),
                    Err(_) => None,
                },
                Some(Err(_)) => Some((
                    Err(std::io::Error::other("Notebook stream interrupted")),
                    (body, revoked, lease),
                )),
                None => None,
            }
        },
    );
    Response::from_parts(parts, Body::from_stream(stream))
}
fn url_query(query: &str) -> Vec<(String, String)> {
    query
        .split('&')
        .filter_map(|s| s.split_once('='))
        .map(|(k, v)| {
            (
                percent_encoding::percent_decode_str(k)
                    .decode_utf8_lossy()
                    .into_owned(),
                percent_encoding::percent_decode_str(v)
                    .decode_utf8_lossy()
                    .into_owned(),
            )
        })
        .collect()
}
