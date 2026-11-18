//! Private outbound node relay and reservation-authorized HTTP/WebSocket gateway.
pub mod authority;
use axum::{
    Router,
    body::{Body, to_bytes},
    extract::{
        FromRequestParts, State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    http::{HeaderMap, HeaderValue, Request, Response, StatusCode, header},
    response::IntoResponse,
    routing::{get, post},
};
use cbgpu_notebook_protocol::{
    Challenge, Frame, MAX_HTTP_BYTES, MAX_RELAY_MESSAGE, MAX_WS_BYTES, decode_body, decode_frame,
    encode_body, encode_frame, headers_valid, notebook_uri, uuid,
};
use ed25519_dalek::SigningKey;
use futures_util::{SinkExt, StreamExt};
use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::sync::{Mutex, OwnedSemaphorePermit, Semaphore, mpsc, watch};
pub type Result<T> = std::result::Result<T, Error>;
#[derive(Debug)]
pub enum Error {
    Denied,
    Unavailable,
    Busy,
    Invalid,
}
impl IntoResponse for Error {
    fn into_response(self) -> axum::response::Response {
        let (status, text) = match self {
            Self::Denied => (
                StatusCode::FORBIDDEN,
                "Notebook access expired or unavailable. Open it from your reservation again.",
            ),
            Self::Unavailable => (
                StatusCode::SERVICE_UNAVAILABLE,
                "Notebook connection is temporarily unavailable.",
            ),
            Self::Busy => (
                StatusCode::TOO_MANY_REQUESTS,
                "Notebook connection is busy. Try again.",
            ),
            Self::Invalid => (StatusCode::BAD_REQUEST, "Invalid notebook request."),
        };
        let mut r = (status, text).into_response();
        r.headers_mut().insert(
            header::CACHE_CONTROL,
            HeaderValue::from_static("private, no-store"),
        );
        r
    }
}
pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|n| n.as_secs() as i64)
        .unwrap_or(0)
}
struct Peer {
    generation: uuid::Uuid,
    version: i64,
    tx: mpsc::Sender<Frame>,
    streams: std::sync::Mutex<HashMap<String, mpsc::Sender<Frame>>>,
    job_slots: std::sync::Mutex<HashMap<String, std::sync::Weak<Semaphore>>>,
    stop: watch::Sender<bool>,
    slots: Arc<Semaphore>,
}
pub struct Gateway {
    pub authority: Arc<dyn authority::Authority>,
    pub signer: SigningKey,
    peers: Mutex<HashMap<String, Arc<Peer>>>,
    slots: Arc<Semaphore>,
    pub recheck: Duration,
    pub bootstrap_key: String,
    bootstraps: Mutex<HashMap<String, Bootstrap>>,
}
impl Gateway {
    pub fn new(authority: Arc<dyn authority::Authority>, signer: SigningKey) -> Arc<Self> {
        Arc::new(Self {
            authority,
            signer,
            peers: Mutex::new(HashMap::new()),
            slots: Arc::new(Semaphore::new(64)),
            recheck: Duration::from_secs(10),
            bootstrap_key: String::new(),
            bootstraps: Mutex::new(HashMap::new()),
        })
    }
}
pub fn router(g: Arc<Gateway>) -> Router {
    Router::new().route("/health",get(||async{axum::Json(serde_json::json!({"ok":true,"transport":"authenticated-outbound-notebook-relay"}))})).route("/internal/bootstrap",post(mint_bootstrap)).route("/notebooks/{job}/_session",post(redeem_bootstrap)).route("/v1/gpu/relay",get(node_upgrade)).fallback(notebook).with_state(g)
}
async fn node_upgrade(
    State(g): State<Arc<Gateway>>,
    ws: WebSocketUpgrade,
    headers: HeaderMap,
) -> Result<axum::response::Response> {
    let node = headers
        .get("x-cbgpu-node-id")
        .and_then(|s| s.to_str().ok())
        .filter(|s| uuid(s))
        .ok_or(Error::Denied)?
        .to_owned();
    let version = headers
        .get("x-cbgpu-credential-version")
        .and_then(|s| s.to_str().ok())
        .and_then(|s| s.parse::<i64>().ok())
        .filter(|v| *v > 0)
        .ok_or(Error::Denied)?;
    let identity = g.authority.identity(&node).await?;
    if identity.version != version {
        return Err(Error::Denied);
    }
    Ok(ws
        .max_message_size(MAX_RELAY_MESSAGE)
        .max_frame_size(MAX_RELAY_MESSAGE)
        .on_upgrade(move |socket| node_socket(g, node, identity, socket)))
}
async fn send(socket: &mut WebSocket, frame: &Frame) -> Result<()> {
    let text = encode_frame(frame).map_err(|_| Error::Invalid)?;
    socket
        .send(Message::Text(text.into()))
        .await
        .map_err(|_| Error::Unavailable)
}
async fn node_socket(
    g: Arc<Gateway>,
    node: String,
    identity: authority::Identity,
    mut socket: WebSocket,
) {
    let mut nonce = [0; 32];
    nonce[..16].copy_from_slice(uuid::Uuid::new_v4().as_bytes());
    nonce[16..].copy_from_slice(uuid::Uuid::new_v4().as_bytes());
    let c = Challenge::issue(node.clone(), identity.version, nonce, now(), &g.signer);
    if send(
        &mut socket,
        &Frame::Challenge {
            challenge: c.clone(),
        },
    )
    .await
    .is_err()
    {
        return;
    }
    let Ok(Some(Ok(Message::Text(proof)))) =
        tokio::time::timeout(Duration::from_secs(20), socket.next()).await
    else {
        return;
    };
    let Ok(Frame::Auth { signature }) = decode_frame(&proof) else {
        return;
    };
    if !c.verify_node(&identity.key, &signature, now()) {
        return;
    }
    match g.authority.identity(&node).await {
        Ok(v) if v.version == identity.version && v.key == identity.key => {}
        _ => return,
    }
    let (tx, mut rx) = mpsc::channel(4);
    let (stop, mut stopping) = watch::channel(false);
    let peer = Arc::new(Peer {
        generation: uuid::Uuid::new_v4(),
        version: identity.version,
        tx,
        streams: std::sync::Mutex::new(HashMap::new()),
        job_slots: std::sync::Mutex::new(HashMap::new()),
        stop,
        slots: Arc::new(Semaphore::new(32)),
    });
    let old = {
        let mut peers = g.peers.lock().await;
        if g.authority
            .relay(&node, identity.version, true)
            .await
            .is_err()
        {
            return;
        }
        peers.insert(node.clone(), peer.clone())
    };
    if let Some(old) = old {
        let _ = old.stop.send(true);
    }
    if send(&mut socket, &Frame::Ready {}).await.is_err() {
        cleanup_peer(&g, &node, &peer).await;
        return;
    }
    let mut ping = tokio::time::interval(Duration::from_secs(20));
    ping.tick().await;
    let mut last_pong = tokio::time::Instant::now();
    loop {
        tokio::select! {
         _=stopping.changed()=>break,
         _=ping.tick()=>{if last_pong.elapsed()>Duration::from_secs(50){break}match g.authority.identity(&node).await{Ok(v)if v.version==peer.version&&v.key==identity.key=>{},Err(Error::Denied)|Ok(_)=>{let _=send(&mut socket,&Frame::Revoked{}).await;break},Err(_)=>break}if send(&mut socket,&Frame::Ping{}).await.is_err(){break}},
         outgoing=rx.recv()=>{let Some(frame)=outgoing else{break};if send(&mut socket,&frame).await.is_err(){break}},
         incoming=socket.next()=>{let Some(Ok(message))=incoming else{break};match message{
          Message::Text(text)=>{let Ok(frame)=decode_frame(&text)else{break};match frame{Frame::Pong{}=>{last_pong=tokio::time::Instant::now();if g.authority.relay(&node,peer.version,true).await.is_err(){break}},Frame::Ping{}=>{if send(&mut socket,&Frame::Pong{}).await.is_err(){break}},Frame::HttpResponse{ref stream_id,..}|Frame::WsOpened{ref stream_id,..}|Frame::WsData{ref stream_id,..}|Frame::Close{ref stream_id,..}|Frame::Error{ref stream_id,..}=>{let key=stream_id.clone();let sender=peer.streams.lock().unwrap_or_else(|e|e.into_inner()).get(&key).cloned();if let Some(sender)=sender&&sender.try_send(frame).is_err(){peer.streams.lock().unwrap_or_else(|e|e.into_inner()).remove(&key);let _=send(&mut socket,&Frame::Close{stream_id:key,code:1013}).await;}},_=>break}},
          Message::Ping(bytes)=>{if socket.send(Message::Pong(bytes)).await.is_err(){break}},Message::Pong(_)=>{},Message::Close(_)=>break,_=>break
         }}
        }
    }
    cleanup_peer(&g, &node, &peer).await;
    let _ = socket.close().await;
}
async fn cleanup_peer(g: &Arc<Gateway>, node: &str, peer: &Arc<Peer>) {
    let _ = peer.stop.send(true);
    peer.streams
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clear();
    // Serialize persisted disconnect with replacement registration, not just map removal.
    let mut peers = g.peers.lock().await;
    if peers
        .get(node)
        .is_some_and(|p| p.generation == peer.generation)
    {
        peers.remove(node);
        let _ = g.authority.relay(node, peer.version, false).await;
    }
}

const NOTEBOOK_HOST: &str = "notebooks.cbgpu.com";
const PARENT_ORIGINS: &[&str] = &[
    "https://cbgpu.com",
    "https://www.cbgpu.com",
    "https://app.cbgpu.com",
    "https://student.cbgpu.com",
    "https://researcher.cbgpu.com",
    "https://developer.cbgpu.com",
];
fn allowed_host(headers: &HeaderMap) -> Result<String> {
    let host = headers
        .get("x-cbgpu-public-host")
        .or_else(|| headers.get(header::HOST))
        .and_then(|v| v.to_str().ok())
        .ok_or(Error::Denied)?;
    if host != NOTEBOOK_HOST {
        return Err(Error::Denied);
    }
    Ok(host.into())
}
struct Bootstrap {
    token: String,
    job: String,
    parent: String,
    expires: i64,
}
async fn mint_bootstrap(
    State(g): State<Arc<Gateway>>,
    request: Request<Body>,
) -> Result<axum::response::Response> {
    let supplied = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if g.bootstrap_key.len() < 32 || supplied != format!("Bearer {}", g.bootstrap_key) {
        return Err(Error::Denied);
    }
    let bytes = to_bytes(request.into_body(), 4096)
        .await
        .map_err(|_| Error::Invalid)?;
    let v: serde_json::Value = serde_json::from_slice(&bytes).map_err(|_| Error::Invalid)?;
    let job = v["job_id"]
        .as_str()
        .filter(|v| uuid(v))
        .ok_or(Error::Invalid)?;
    let token = v["cookie_token"]
        .as_str()
        .filter(|v| v.len() == 64 && v.bytes().all(|b| b.is_ascii_hexdigit()))
        .ok_or(Error::Invalid)?;
    let parent = v["parent_origin"]
        .as_str()
        .filter(|v| PARENT_ORIGINS.contains(v))
        .ok_or(Error::Denied)?;
    g.authority.grant(token, job).await?;
    let code = format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    );
    let expires = now() + 60;
    let mut entries = g.bootstraps.lock().await;
    entries.retain(|_, v| v.expires > now());
    if entries.len() >= 1024 {
        return Err(Error::Busy);
    }
    entries.insert(
        code.clone(),
        Bootstrap {
            token: token.into(),
            job: job.into(),
            parent: parent.into(),
            expires,
        },
    );
    let mut response=axum::Json(serde_json::json!({"bootstrap_token":code,"expires_at":expires,"notebook_origin":"https://notebooks.cbgpu.com"})).into_response();
    secure_headers(response.headers_mut());
    Ok(response)
}
async fn redeem_bootstrap(
    State(g): State<Arc<Gateway>>,
    axum::extract::Path(job): axum::extract::Path<String>,
    request: Request<Body>,
) -> Result<axum::response::Response> {
    if !uuid(&job) {
        return Err(Error::Invalid);
    }
    allowed_host(request.headers())?;
    let parent = request
        .headers()
        .get(header::ORIGIN)
        .and_then(|v| v.to_str().ok())
        .filter(|v| PARENT_ORIGINS.contains(v))
        .ok_or(Error::Denied)?
        .to_owned();
    if !request
        .headers()
        .get(header::CONTENT_TYPE)
        .is_some_and(|v| {
            v.as_bytes()
                .starts_with(b"application/x-www-form-urlencoded")
        })
    {
        return Err(Error::Invalid);
    }
    let bytes = to_bytes(request.into_body(), 512)
        .await
        .map_err(|_| Error::Invalid)?;
    let fields = url::form_urlencoded::parse(&bytes).collect::<Vec<_>>();
    if fields.len() != 1 || fields[0].0 != "bootstrap_token" {
        return Err(Error::Invalid);
    }
    let code = &fields[0].1;
    let entry = {
        let mut map = g.bootstraps.lock().await;
        let valid = map
            .get(code.as_ref())
            .is_some_and(|v| v.job == job && v.parent == parent && v.expires > now());
        if !valid {
            return Err(Error::Denied);
        }
        map.remove(code.as_ref()).ok_or(Error::Denied)?
    };
    let grant = g.authority.grant(&entry.token, &job).await?;
    let lifetime = (grant.expires_at - now()).clamp(0, 14400);
    if lifetime == 0 {
        return Err(Error::Denied);
    }
    let mut response=Response::builder().status(303).header(header::LOCATION,format!("/notebooks/{job}/lab")).header(header::SET_COOKIE,format!("__Secure-cbgpu-notebook={}; Path=/notebooks/{job}/; Max-Age={lifetime}; Secure; HttpOnly; SameSite=Lax",entry.token)).body(Body::empty()).map_err(|_|Error::Invalid)?;
    secure_headers(response.headers_mut());
    Ok(response.into_response())
}
fn cookie(headers: &HeaderMap) -> Result<String> {
    let value = headers
        .get(header::COOKIE)
        .and_then(|s| s.to_str().ok())
        .filter(|s| s.len() <= 16384)
        .ok_or(Error::Denied)?;
    let mut values = value
        .split(';')
        .filter_map(|part| part.trim().strip_prefix("__Secure-cbgpu-notebook="));
    let token = values
        .next()
        .filter(|s| {
            s.len() >= 16
                && s.len() <= 256
                && s.bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c))
        })
        .ok_or(Error::Denied)?;
    if values.next().is_some() {
        return Err(Error::Denied);
    }
    Ok(token.into())
}
fn request_headers(headers: &HeaderMap, host: &str) -> Vec<(String, String)> {
    let mut result = vec![
        ("Host".into(), host.into()),
        ("X-Forwarded-Proto".into(), "https".into()),
    ];
    for key in [
        "accept",
        "accept-encoding",
        "accept-language",
        "content-type",
        "if-none-match",
        "if-modified-since",
        "range",
        "origin",
        "x-xsrftoken",
    ] {
        if let Some(value) = headers
            .get(key)
            .and_then(|v| v.to_str().ok())
            .filter(|v| v.len() <= 8192)
        {
            result.push((key.into(), value.into()))
        }
    }
    if let Some(cookies) = headers.get(header::COOKIE).and_then(|v| v.to_str().ok()) {
        let xsrf = cookies
            .split(';')
            .find_map(|v| v.trim().strip_prefix("_xsrf="));
        if let Some(xsrf) = xsrf.filter(|v| v.len() <= 4096) {
            result.push(("Cookie".into(), format!("_xsrf={xsrf}")))
        }
    }
    result
}
struct Stream {
    peer: Arc<Peer>,
    id: String,
    rx: mpsc::Receiver<Frame>,
    closed: bool,
    _global: OwnedSemaphorePermit,
    _node: OwnedSemaphorePermit,
    _job: OwnedSemaphorePermit,
}
impl Stream {
    fn remove(&mut self) -> bool {
        if self.closed {
            return false;
        }
        self.closed = true;
        self.peer
            .streams
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.id);
        true
    }
    async fn finish(&mut self) {
        if self.remove()
            && !matches!(
                tokio::time::timeout(
                    Duration::from_secs(1),
                    self.peer.tx.send(Frame::Close {
                        stream_id: self.id.clone(),
                        code: 1000
                    })
                )
                .await,
                Ok(Ok(()))
            )
        {
            let _ = self.peer.stop.send(true);
        }
    }
}
impl Drop for Stream {
    fn drop(&mut self) {
        if self.remove()
            && self
                .peer
                .tx
                .try_send(Frame::Close {
                    stream_id: self.id.clone(),
                    code: 1000,
                })
                .is_err()
        {
            // Backpressure cannot orphan a node-side stream: close the authenticated peer.
            let _ = self.peer.stop.send(true);
        }
    }
}
async fn stream(g: &Arc<Gateway>, grant: &authority::Grant) -> Result<Stream> {
    let global = g
        .slots
        .clone()
        .try_acquire_owned()
        .map_err(|_| Error::Busy)?;
    let peer = g
        .peers
        .lock()
        .await
        .get(&grant.node_id)
        .cloned()
        .filter(|p| p.version == grant.version)
        .ok_or(Error::Unavailable)?;
    let node = peer
        .slots
        .clone()
        .try_acquire_owned()
        .map_err(|_| Error::Busy)?;
    let job_slots = {
        let mut jobs = peer.job_slots.lock().unwrap_or_else(|e| e.into_inner());
        jobs.retain(|_, slots| slots.strong_count() > 0);
        match jobs.get(&grant.job_id).and_then(std::sync::Weak::upgrade) {
            Some(slots) => slots,
            None => {
                let slots = Arc::new(Semaphore::new(8));
                jobs.insert(grant.job_id.clone(), Arc::downgrade(&slots));
                slots
            }
        }
    };
    let job = job_slots.try_acquire_owned().map_err(|_| Error::Busy)?;
    let (tx, rx) = mpsc::channel(4);
    let id = uuid::Uuid::new_v4().to_string();
    {
        let mut streams = peer.streams.lock().unwrap_or_else(|e| e.into_inner());
        if *peer.stop.borrow() {
            return Err(Error::Unavailable);
        }
        streams.insert(id.clone(), tx);
    }
    Ok(Stream {
        peer,
        id,
        rx,
        closed: false,
        _global: global,
        _node: node,
        _job: job,
    })
}

async fn notebook(
    State(g): State<Arc<Gateway>>,
    request: Request<Body>,
) -> axum::response::Response {
    match notebook_inner(g, request).await {
        Ok(r) => r,
        Err(e) => e.into_response(),
    }
}
async fn notebook_inner(
    g: Arc<Gateway>,
    request: Request<Body>,
) -> Result<axum::response::Response> {
    let (mut parts, body) = request.into_parts();
    let job = parts
        .uri
        .path()
        .split('/')
        .nth(2)
        .filter(|id| uuid(id))
        .ok_or(Error::Invalid)?
        .to_owned();
    let uri = parts.uri.to_string();
    if !notebook_uri(&job, &uri) {
        return Err(Error::Invalid);
    }
    let host = allowed_host(&parts.headers)?;
    let origin = parts
        .headers
        .get(header::ORIGIN)
        .and_then(|v| v.to_str().ok());
    if origin.is_some_and(|o| o != format!("https://{host}")) {
        return Err(Error::Denied);
    }
    let upgrade = parts
        .headers
        .get(header::UPGRADE)
        .is_some_and(|v| v.as_bytes().eq_ignore_ascii_case(b"websocket"));
    if (!matches!(parts.method.as_str(), "GET" | "HEAD") || upgrade)
        && origin != Some(format!("https://{host}").as_str())
    {
        return Err(Error::Denied);
    }
    let token = cookie(&parts.headers)?;
    let grant = g.authority.grant(&token, &job).await?;
    let mut s = stream(&g, &grant).await?;
    let headers = request_headers(&parts.headers, &host);
    if upgrade {
        let ws = WebSocketUpgrade::from_request_parts(&mut parts, &())
            .await
            .map_err(|_| Error::Invalid)?;
        let protocols = parts
            .headers
            .get(header::SEC_WEBSOCKET_PROTOCOL)
            .and_then(|v| v.to_str().ok())
            .map(|v| {
                v.split(',')
                    .map(|s| s.trim().to_owned())
                    .filter(|s| s.len() <= 128)
                    .take(8)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        s.peer
            .tx
            .send(Frame::WsOpen {
                stream_id: s.id.clone(),
                job_id: job.clone(),
                relay_session_id: grant.relay_session_id.clone(),
                uri,
                headers,
                protocols: protocols.clone(),
            })
            .await
            .map_err(|_| Error::Unavailable)?;
        let first = tokio::time::timeout(Duration::from_secs(15), s.rx.recv())
            .await
            .map_err(|_| Error::Unavailable)?
            .ok_or(Error::Unavailable)?;
        let Frame::WsOpened { protocol, .. } = first else {
            s.finish().await;
            return Err(Error::Unavailable);
        };
        let selected = protocol.filter(|p| protocols.contains(p));
        let ws = ws
            .max_message_size(MAX_WS_BYTES)
            .max_frame_size(MAX_WS_BYTES);
        let ws = if let Some(p) = selected {
            ws.protocols([p])
        } else {
            ws
        };
        return Ok(ws.on_upgrade(move |socket| browser_socket(g, grant, token, s, socket)));
    }
    if !["GET", "HEAD", "POST", "PUT", "PATCH", "DELETE", "OPTIONS"]
        .contains(&parts.method.as_str())
    {
        s.finish().await;
        return Err(Error::Invalid);
    }
    let bytes = match to_bytes(body, MAX_HTTP_BYTES).await {
        Ok(v) => v,
        Err(_) => {
            s.finish().await;
            return Ok((
                StatusCode::PAYLOAD_TOO_LARGE,
                "Notebook request is too large; upload in smaller chunks.",
            )
                .into_response());
        }
    };
    let frame = Frame::HttpRequest {
        stream_id: s.id.clone(),
        job_id: job.clone(),
        relay_session_id: grant.relay_session_id,
        method: parts.method.to_string(),
        uri,
        headers,
        body: encode_body(&bytes),
    };
    if s.peer.tx.send(frame).await.is_err() {
        s.finish().await;
        return Err(Error::Unavailable);
    }
    let result = tokio::time::timeout(Duration::from_secs(60), s.rx.recv()).await;
    s.finish().await;
    let Ok(Some(Frame::HttpResponse {
        status,
        headers,
        body,
        ..
    })) = result
    else {
        return Err(Error::Unavailable);
    };
    if !(200..=599).contains(&status) || !headers_valid(&headers) {
        return Err(Error::Unavailable);
    }
    let bytes = decode_body(&body, MAX_HTTP_BYTES).ok_or(Error::Unavailable)?;
    let mut response = Response::builder().status(status);
    for (key, value) in headers {
        let lower = key.to_ascii_lowercase();
        if [
            "content-type",
            "content-encoding",
            "content-disposition",
            "etag",
            "last-modified",
            "accept-ranges",
            "content-range",
        ]
        .contains(&lower.as_str())
        {
            response = response.header(key, value)
        } else if lower == "location"
            && notebook_uri(&job, &value)
            && value.starts_with(&format!("/notebooks/{job}/"))
        {
            response = response.header("location", value)
        } else if lower == "set-cookie"
            && let Some(raw) = value.split(';').next().filter(|v| {
                v.starts_with("_xsrf=") && v.len() <= 4096 && !v.chars().any(char::is_control)
            })
        {
            response = response.header(
                "set-cookie",
                format!("{raw}; Path=/notebooks/{job}/; Secure; SameSite=Lax"),
            )
        }
    }
    let mut response = response
        .body(Body::from(bytes))
        .map_err(|_| Error::Unavailable)?;
    secure_headers(response.headers_mut());
    Ok(response.into_response())
}
fn secure_headers(headers: &mut HeaderMap) {
    for (k, v) in [
        ("cache-control", "private, no-store"),
        ("referrer-policy", "no-referrer"),
        ("x-content-type-options", "nosniff"),
        ("cross-origin-resource-policy", "same-origin"),
        (
            "content-security-policy",
            "frame-ancestors https://cbgpu.com https://www.cbgpu.com https://app.cbgpu.com https://student.cbgpu.com https://researcher.cbgpu.com https://developer.cbgpu.com; base-uri 'self'; object-src 'none'",
        ),
        ("x-robots-tag", "noindex, nofollow"),
    ] {
        headers.insert(
            axum::http::HeaderName::from_static(k),
            HeaderValue::from_static(v),
        );
    }
}
async fn browser_socket(
    g: Arc<Gateway>,
    grant: authority::Grant,
    token: String,
    mut s: Stream,
    mut socket: WebSocket,
) {
    let mut recheck = tokio::time::interval(g.recheck);
    recheck.tick().await;
    let mut stopping = s.peer.stop.subscribe();
    loop {
        tokio::select! {
        _=stopping.changed()=>break,
        _=recheck.tick()=>{match g.authority.grant(&token,&grant.job_id).await{Ok(new)if new.node_id==grant.node_id&&new.version==grant.version&&new.relay_session_id==grant.relay_session_id=>{},_=>break}},
        message=socket.next()=>{let Some(Ok(message))=message else{break};let frame=match message{Message::Text(text)=>{if text.len()>MAX_WS_BYTES{break}Frame::WsData{stream_id:s.id.clone(),binary:false,body:encode_body(text.as_bytes())}},Message::Binary(bytes)=>{if bytes.len()>MAX_WS_BYTES{break}Frame::WsData{stream_id:s.id.clone(),binary:true,body:encode_body(&bytes)}},Message::Ping(bytes)=>{if socket.send(Message::Pong(bytes)).await.is_err(){break}continue},Message::Pong(_)=>continue,Message::Close(_)=>break};if s.peer.tx.send(frame).await.is_err(){break}},
        frame=s.rx.recv()=>{match frame{Some(Frame::WsData{binary,body,..})=>{let Some(bytes)=decode_body(&body,MAX_WS_BYTES)else{break};let message=if binary{Message::Binary(bytes.into())}else{let Ok(text)=String::from_utf8(bytes)else{break};Message::Text(text.into())};if socket.send(message).await.is_err(){break}},_=>break}}
        }
    }
    s.finish().await;
    let _ = socket.close().await;
}
