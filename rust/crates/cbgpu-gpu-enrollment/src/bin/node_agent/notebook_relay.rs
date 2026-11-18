//! Outbound authenticated relay. All upstreams come from the verified local Docker journal.
use super::{
    notebook_executor::{Docker, Executor},
    notebook_http,
};
use cbgpu_notebook_protocol::{
    Frame, MAX_HTTP_BYTES, MAX_RELAY_MESSAGE, MAX_WS_BYTES, decode_body, decode_frame, encode_body,
    encode_frame, headers_valid, notebook_uri, unhex, uuid,
};
use ed25519_dalek::{SigningKey, VerifyingKey};
use futures_util::{SinkExt, StreamExt};
use std::{collections::BTreeMap, sync::Arc};
use tokio::sync::{Mutex, Semaphore, mpsc};
use tokio_tungstenite::tungstenite::{
    Message, client::IntoClientRequest, protocol::WebSocketConfig,
};
type Error = &'static str;
type Result<T> = std::result::Result<T, Error>;
fn now() -> Result<i64> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| "CLOCK_UNAVAILABLE")?
        .as_secs()
        .try_into()
        .map_err(|_| "CLOCK_UNAVAILABLE")
}
fn message(frame: &Frame) -> Result<Message> {
    Ok(Message::Text(
        encode_frame(frame)
            .map_err(|_| "INVALID_RELAY_FRAME")?
            .into(),
    ))
}
fn safe_headers(headers: &[(String, String)]) -> Result<Vec<(String, String)>> {
    if !headers_valid(headers) {
        return Err("INVALID_RELAY_HEADERS");
    }
    let mut out = vec![];
    for (k, v) in headers {
        let lower = k.to_ascii_lowercase();
        if [
            "accept",
            "accept-language",
            "content-type",
            "if-none-match",
            "if-modified-since",
            "range",
            "x-xsrftoken",
            "x-requested-with",
        ]
        .contains(&lower.as_str())
        {
            out.push((lower, v.clone()));
        } else if lower == "cookie" {
            let xsrf = v
                .split(';')
                .map(str::trim)
                .filter(|c| c.starts_with("_xsrf="))
                .collect::<Vec<_>>()
                .join("; ");
            if !xsrf.is_empty() {
                out.push((lower, xsrf));
            }
        }
    }
    Ok(out)
}
fn response_headers(headers: &reqwest::header::HeaderMap, job: &str) -> Vec<(String, String)> {
    let mut output: Vec<(String, String)> = headers
        .iter()
        .filter_map(|(k, v)| {
            let k = k.as_str();
            if ![
                "content-type",
                "content-disposition",
                "content-encoding",
                "etag",
                "last-modified",
                "cache-control",
                "vary",
                "accept-ranges",
                "content-range",
            ]
            .contains(&k)
            {
                return None;
            }
            let value = v.to_str().ok()?;
            if value.len() > 8192 {
                return None;
            }
            Some((k.into(), value.into()))
        })
        .take(24)
        .collect();
    for value in headers.get_all("set-cookie") {
        if let Ok(value) = value.to_str()
            && let Some(cookie) = value
                .split(';')
                .next()
                .filter(|v| v.starts_with("_xsrf=") && v.len() <= 8192)
        {
            output.push((
                "set-cookie".into(),
                format!("{cookie}; Path=/notebooks/{job}/; Secure; SameSite=Lax"),
            ));
        }
    }
    output
}
async fn http<D: Docker>(executor: Arc<Executor<D>>, frame: Frame) -> Result<Frame> {
    let Frame::HttpRequest {
        stream_id: stream,
        job_id: job,
        relay_session_id: session,
        method,
        uri,
        headers,
        body,
    } = frame
    else {
        return Err("INVALID_RELAY_FRAME");
    };
    if !notebook_uri(&job, &uri)
        || !["GET", "HEAD", "POST", "PUT", "PATCH", "DELETE", "OPTIONS"].contains(&method.as_str())
    {
        return Err("INVALID_NOTEBOOK_REQUEST");
    }
    let bytes = decode_body(&body, MAX_HTTP_BYTES).ok_or("RELAY_BODY_TOO_LARGE")?;
    let (pipe, token) = executor.relay_stream(&job, &session, now()?).await?;
    let mut request = hyper::Request::builder()
        .method(method.as_str())
        .uri(&uri)
        .header("Host", "localhost")
        .header("Authorization", format!("token {token}"));
    for (k, v) in safe_headers(&headers)? {
        request = request.header(k, v);
    }
    let request = request
        .body(http_body_util::Full::new(bytes::Bytes::from(bytes)))
        .map_err(|_| "INVALID_NOTEBOOK_REQUEST")?;
    let response = tokio::time::timeout(
        std::time::Duration::from_secs(45),
        notebook_http::request(pipe, request),
    )
    .await
    .map_err(|_| "UPSTREAM_UNAVAILABLE")??;
    let status = response.status().as_u16();
    let headers = response_headers(response.headers(), &job);
    let bytes = tokio::time::timeout(
        std::time::Duration::from_secs(45),
        notebook_http::body(response.into_body(), MAX_HTTP_BYTES),
    )
    .await
    .map_err(|_| "UPSTREAM_UNAVAILABLE")??;
    executor.relay_upstream(&job, &session, now()?).await?;
    Ok(Frame::HttpResponse {
        stream_id: stream,
        status,
        headers,
        body: encode_body(&bytes),
    })
}
async fn ws<D: Docker>(
    executor: Arc<Executor<D>>,
    frame: Frame,
    mut inbound: mpsc::Receiver<Frame>,
    out: mpsc::Sender<Frame>,
) -> Result<()> {
    let Frame::WsOpen {
        stream_id: stream,
        job_id: job,
        relay_session_id: session,
        uri,
        headers,
        protocols,
    } = frame
    else {
        return Err("INVALID_RELAY_FRAME");
    };
    if !notebook_uri(&job, &uri)
        || protocols.len() > 8
        || protocols.iter().any(|p| {
            p.len() > 128
                || !p
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b))
        })
    {
        return Err("INVALID_NOTEBOOK_REQUEST");
    }
    let (pipe, token) = executor.relay_stream(&job, &session, now()?).await?;
    let mut req = format!("ws://localhost{uri}")
        .into_client_request()
        .map_err(|_| "INVALID_NOTEBOOK_REQUEST")?;
    for (k, v) in safe_headers(&headers)? {
        req.headers_mut().insert(
            tokio_tungstenite::tungstenite::http::header::HeaderName::from_bytes(k.as_bytes())
                .map_err(|_| "INVALID_RELAY_HEADERS")?,
            v.parse().map_err(|_| "INVALID_RELAY_HEADERS")?,
        );
    }
    req.headers_mut().insert(
        "authorization",
        format!("token {token}")
            .parse()
            .map_err(|_| "INVALID_RELAY_HEADERS")?,
    );
    if !protocols.is_empty() {
        req.headers_mut().insert(
            "sec-websocket-protocol",
            protocols
                .join(", ")
                .parse()
                .map_err(|_| "INVALID_RELAY_HEADERS")?,
        );
    }
    let config = WebSocketConfig::default()
        .max_message_size(Some(MAX_WS_BYTES))
        .max_frame_size(Some(MAX_WS_BYTES));
    let (mut socket, response) =
        tokio_tungstenite::client_async_with_config(req, pipe, Some(config))
            .await
            .map_err(|_| "UPSTREAM_UNAVAILABLE")?;
    let protocol = response
        .headers()
        .get("sec-websocket-protocol")
        .and_then(|p| p.to_str().ok())
        .map(str::to_owned);
    if protocol.as_ref().is_some_and(|p| !protocols.contains(p)) {
        return Err("INVALID_UPSTREAM_PROTOCOL");
    }
    out.send(Frame::WsOpened {
        stream_id: stream.clone(),
        protocol,
    })
    .await
    .map_err(|_| "RELAY_DISCONNECTED")?;
    let mut interval = tokio::time::interval(std::time::Duration::from_secs(1));
    loop {
        tokio::select! {_ =interval.tick()=>{executor.relay_upstream(&job,&session,now()?).await?;},frame=inbound.recv()=>{match frame{Some(Frame::WsData{binary,body,..})=>{executor.relay_upstream(&job,&session,now()?).await?;let bytes=decode_body(&body,MAX_WS_BYTES).ok_or("RELAY_BODY_TOO_LARGE")?;let value=if binary{Message::Binary(bytes.into())}else{Message::Text(String::from_utf8(bytes).map_err(|_|"INVALID_WS_TEXT")?.into())};socket.send(value).await.map_err(|_|"UPSTREAM_UNAVAILABLE")?;},Some(Frame::Close{..})|None=>{let _=socket.close(None).await;return Ok(())},_=>return Err("INVALID_RELAY_FRAME")}},frame=socket.next()=>{match frame{Some(Ok(Message::Binary(bytes)))=>out.send(Frame::WsData{stream_id:stream.clone(),binary:true,body:encode_body(&bytes)}).await.map_err(|_|"RELAY_DISCONNECTED")?,Some(Ok(Message::Text(text)))=>out.send(Frame::WsData{stream_id:stream.clone(),binary:false,body:encode_body(text.as_bytes())}).await.map_err(|_|"RELAY_DISCONNECTED")?,Some(Ok(Message::Ping(bytes)))=>{socket.send(Message::Pong(bytes)).await.map_err(|_|"UPSTREAM_UNAVAILABLE")?;},Some(Ok(Message::Pong(_)))=>{},Some(Ok(Message::Close(_)))|None=>return Ok(()),_=>return Err("UPSTREAM_UNAVAILABLE")}}}
    }
}
struct Stream {
    job: String,
    inbound: mpsc::Sender<Frame>,
}
pub async fn connect<D: Docker + 'static>(
    origin: &str,
    node: &str,
    version: i64,
    key: &SigningKey,
    server_key: &str,
    executor: Arc<Executor<D>>,
) -> Result<()> {
    let mut url = url::Url::parse(origin).map_err(|_| "INVALID_REGISTRY")?;
    if url.scheme() != "https"
        || url.host_str().is_none()
        || url.port_or_known_default() != Some(443)
        || url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err("INVALID_REGISTRY");
    }
    url.set_scheme("wss").map_err(|_| "INVALID_REGISTRY")?;
    url.set_path("/v1/gpu/relay");
    let mut request = url
        .as_str()
        .into_client_request()
        .map_err(|_| "INVALID_REGISTRY")?;
    request
        .headers_mut()
        .insert("x-cbgpu-node-id", node.parse().map_err(|_| "INVALID_NODE")?);
    request.headers_mut().insert(
        "x-cbgpu-credential-version",
        version.to_string().parse().map_err(|_| "INVALID_NODE")?,
    );
    let config = WebSocketConfig::default()
        .max_message_size(Some(MAX_RELAY_MESSAGE))
        .max_frame_size(Some(MAX_RELAY_MESSAGE));
    let (mut socket, _) =
        tokio_tungstenite::connect_async_with_config(request, Some(config), false)
            .await
            .map_err(|_| "RELAY_UNAVAILABLE")?;
    let Some(Ok(Message::Text(text))) =
        tokio::time::timeout(std::time::Duration::from_secs(15), socket.next())
            .await
            .map_err(|_| "RELAY_UNAVAILABLE")?
    else {
        return Err("INVALID_RELAY_HANDSHAKE");
    };
    let Frame::Challenge { challenge } =
        decode_frame(&text).map_err(|_| "INVALID_RELAY_HANDSHAKE")?
    else {
        return Err("INVALID_RELAY_HANDSHAKE");
    };
    let central = VerifyingKey::from_bytes(&unhex::<32>(server_key).ok_or("INVALID_SERVER_TRUST")?)
        .map_err(|_| "INVALID_SERVER_TRUST")?;
    let time = now()?;
    if challenge.node_id != node
        || challenge.credential_version != version
        || !challenge.verify_server(&central, time)
        || challenge.issued_at < time - 30
        || challenge.issued_at > time + 30
        || challenge.expires_at <= time
    {
        return Err("INVALID_RELAY_AUTHORITY");
    }
    socket
        .send(message(&Frame::Auth {
            signature: challenge.node_signature(key),
        })?)
        .await
        .map_err(|_| "RELAY_UNAVAILABLE")?;
    let Some(Ok(Message::Text(text))) =
        tokio::time::timeout(std::time::Duration::from_secs(15), socket.next())
            .await
            .map_err(|_| "RELAY_UNAVAILABLE")?
    else {
        return Err("INVALID_RELAY_HANDSHAKE");
    };
    if !matches!(
        decode_frame(&text).map_err(|_| "INVALID_RELAY_HANDSHAKE")?,
        Frame::Ready {}
    ) {
        return Err("INVALID_RELAY_HANDSHAKE");
    }
    let (sink, mut source) = socket.split();
    let (out, mut send) = mpsc::channel::<Frame>(32);
    let writer = tokio::spawn(async move {
        let mut sink = sink;
        while let Some(frame) = send.recv().await {
            sink.send(message(&frame)?)
                .await
                .map_err(|_| "RELAY_DISCONNECTED")?;
        }
        Ok::<(), Error>(())
    });
    let streams = Arc::new(Mutex::new(BTreeMap::<String, Stream>::new()));
    let permits = Arc::new(Semaphore::new(32));
    let mut tasks = tokio::task::JoinSet::new();
    let result = async {
        while let Some(frame) = source.next().await {
            let frame = match frame.map_err(|_| "RELAY_DISCONNECTED")? {
                Message::Text(text) => decode_frame(&text).map_err(|_| "INVALID_RELAY_FRAME")?,
                Message::Ping(_) => {
                    out.send(Frame::Pong {})
                        .await
                        .map_err(|_| "RELAY_DISCONNECTED")?;
                    continue;
                }
                Message::Pong(_) => continue,
                Message::Close(_) => break,
                _ => return Err("INVALID_RELAY_FRAME"),
            };
            match frame {
                Frame::HttpRequest {
                    stream_id,
                    job_id,
                    relay_session_id,
                    method,
                    uri,
                    headers,
                    body,
                } => {
                    if !uuid(&stream_id) || !uuid(&job_id) || !uuid(&relay_session_id) {
                        return Err("INVALID_RELAY_FRAME");
                    }
                    let permit = permits
                        .clone()
                        .try_acquire_owned()
                        .map_err(|_| "TOO_MANY_STREAMS")?;
                    let mut map = streams.lock().await;
                    if map.contains_key(&stream_id)
                        || map.values().filter(|s| s.job == job_id).count() >= 8
                    {
                        return Err("TOO_MANY_STREAMS");
                    }
                    let (tx, _) = mpsc::channel(1);
                    map.insert(
                        stream_id.clone(),
                        Stream {
                            job: job_id.clone(),
                            inbound: tx,
                        },
                    );
                    drop(map);
                    let e = executor.clone();
                    let tx = out.clone();
                    let state = streams.clone();
                    tasks.spawn(async move {
                        let _permit = permit;
                        let response = http(
                            e,
                            Frame::HttpRequest {
                                stream_id: stream_id.clone(),
                                job_id,
                                relay_session_id,
                                method,
                                uri,
                                headers,
                                body,
                            },
                        )
                        .await
                        .unwrap_or(Frame::Error {
                            stream_id: stream_id.clone(),
                            code: "NOTEBOOK_UPSTREAM_UNAVAILABLE".into(),
                        });
                        let _ = tx.send(response).await;
                        state.lock().await.remove(&stream_id);
                    });
                }
                Frame::WsOpen {
                    stream_id,
                    job_id,
                    relay_session_id,
                    uri,
                    headers,
                    protocols,
                } => {
                    if !uuid(&stream_id) || !uuid(&job_id) || !uuid(&relay_session_id) {
                        return Err("INVALID_RELAY_FRAME");
                    }
                    let permit = permits
                        .clone()
                        .try_acquire_owned()
                        .map_err(|_| "TOO_MANY_STREAMS")?;
                    let mut map = streams.lock().await;
                    if map.contains_key(&stream_id)
                        || map.values().filter(|s| s.job == job_id).count() >= 8
                    {
                        return Err("TOO_MANY_STREAMS");
                    }
                    let (tx, rx) = mpsc::channel(8);
                    map.insert(
                        stream_id.clone(),
                        Stream {
                            job: job_id.clone(),
                            inbound: tx,
                        },
                    );
                    drop(map);
                    let e = executor.clone();
                    let tx = out.clone();
                    let state = streams.clone();
                    tasks.spawn(async move {
                        let _permit = permit;
                        if ws(
                            e,
                            Frame::WsOpen {
                                stream_id: stream_id.clone(),
                                job_id,
                                relay_session_id,
                                uri,
                                headers,
                                protocols,
                            },
                            rx,
                            tx.clone(),
                        )
                        .await
                        .is_err()
                        {
                            let _ = tx
                                .send(Frame::Error {
                                    stream_id: stream_id.clone(),
                                    code: "NOTEBOOK_UPSTREAM_UNAVAILABLE".into(),
                                })
                                .await;
                        }
                        let _ = tx
                            .send(Frame::Close {
                                stream_id: stream_id.clone(),
                                code: 1000,
                            })
                            .await;
                        state.lock().await.remove(&stream_id);
                    });
                }
                Frame::WsData { ref stream_id, .. } | Frame::Close { ref stream_id, .. } => {
                    if let Some(stream) = streams.lock().await.get(stream_id) {
                        stream
                            .inbound
                            .try_send(frame.clone())
                            .map_err(|_| "STREAM_BACKPRESSURE")?;
                    }
                }
                Frame::Ping {} => out
                    .send(Frame::Pong {})
                    .await
                    .map_err(|_| "RELAY_DISCONNECTED")?,
                Frame::Pong {} => {}
                Frame::Revoked {} => {
                    executor.stop_all().await?;
                    return Err("NODE_REVOKED");
                }
                _ => return Err("INVALID_RELAY_FRAME"),
            };
            while tasks.try_join_next().is_some() {}
        }
        Ok(())
    }
    .await;
    tasks.abort_all();
    while tasks.join_next().await.is_some() {}
    drop(out);
    writer.abort();
    result
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cookies_and_auth_never_borrow_cbgpu_identity() {
        let h = safe_headers(&[
            (
                "Cookie".into(),
                "sb-cbgpu-auth-token=SECRET; _xsrf=fixture; other=secret".into(),
            ),
            ("Authorization".into(), "Bearer PRIVATE".into()),
            ("Host".into(), "attacker.invalid".into()),
            ("X-XSRFToken".into(), "fixture".into()),
        ])
        .unwrap();
        assert_eq!(
            h,
            vec![
                ("cookie".into(), "_xsrf=fixture".into()),
                ("x-xsrftoken".into(), "fixture".into())
            ]
        );
        assert!(!format!("{h:?}").contains("SECRET"));
    }
    const JOB: &str = "10000000-0000-4000-8000-000000000001";
    const NODE: &str = "10000000-0000-4000-8000-000000000002";
    const SESSION: &str = "10000000-0000-4000-8000-000000000006";
    const IMAGE: &str =
        "fixture/image@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    struct Fake {
        port: u16,
        value: std::sync::Mutex<Option<serde_json::Value>>,
    }
    impl Docker for Fake {
        async fn open_stream(
            &self,
            _: &str,
        ) -> super::super::notebook_executor::Result<super::super::notebook_docker_stream::Pipe>
        {
            Ok(Box::new(
                tokio::net::TcpStream::connect(("127.0.0.1", self.port))
                    .await
                    .map_err(|_| "UPSTREAM_UNAVAILABLE")?,
            ))
        }

        async fn notebook_ready(
            &self,
            _: &str,
            _: &str,
            _: &str,
        ) -> super::super::notebook_executor::Result<bool> {
            Ok(true)
        }
        async fn gpu_processes(
            &self,
        ) -> super::super::notebook_executor::Result<Vec<super::super::notebook_executor::GpuProcess>>
        {
            Ok(vec![])
        }
        async fn request(
            &self,
            method: reqwest::Method,
            path: &str,
            body: Option<serde_json::Value>,
        ) -> super::super::notebook_executor::Result<Option<serde_json::Value>> {
            use serde_json::json;
            if path == "/info" {
                return Ok(Some(
                    json!({"OSType":"linux","MemTotal":34359738368u64,"NCPU":8,"Runtimes":{"nvidia":{}}}),
                ));
            }
            if path.starts_with("/images/") {
                return Ok(Some(json!({"Os":"linux","RepoDigests":[IMAGE]})));
            }
            if path == "/containers/json?all=false" {
                return Ok(Some(json!([])));
            }
            if path.contains("/create?") {
                let b = body.unwrap();
                *self.value.lock().unwrap() = Some(
                    json!({"Id":"a".repeat(64),"Config":{"Image":b["Image"],"Labels":b["Labels"]},"HostConfig":b["HostConfig"],"Mounts":[{"Type":"bind","Source":b["HostConfig"]["Mounts"][0]["Source"],"Destination":"/usr/local/bin/cbgpu-node-proxy","RW":false}],"State":{"Running":false},"NetworkSettings":{"Ports":{"8888/tcp":[{"HostIp":"127.0.0.1","HostPort":self.port.to_string()}]}}}),
                );
                return Ok(Some(json!({"Id":"a".repeat(64)})));
            }
            if path.ends_with("/json") {
                return Ok(self.value.lock().unwrap().clone());
            }
            if path.ends_with("/start") {
                self.value.lock().unwrap().as_mut().unwrap()["State"]["Running"] = json!(true);
                return Ok(Some(json!({})));
            }
            if path.contains("/stop?") {
                self.value.lock().unwrap().as_mut().unwrap()["State"]["Running"] = json!(false);
                return Ok(Some(json!({})));
            }
            if method == reqwest::Method::DELETE {
                *self.value.lock().unwrap() = None;
                return Ok(Some(json!({})));
            }
            panic!("Unexpected synthetic Docker request")
        }
    }
    async fn runtime(port: u16) -> (std::path::PathBuf, Arc<Executor<Fake>>) {
        use cbgpu_gpu_enrollment::notebooks::protocol::{Limits, NotebookSpec};
        let mut random = [0; 8];
        getrandom::fill(&mut random).unwrap();
        let root = std::env::temp_dir().join(format!(
            "cbgpu-relay-cpu-{}",
            cbgpu_gpu_enrollment::hex(&random)
        ));
        let e = Arc::new(
            Executor::open(
                Arc::new(Fake {
                    port,
                    value: std::sync::Mutex::new(None),
                }),
                &root,
                NODE,
                IMAGE,
            )
            .unwrap()
            .with_test_locks(),
        );
        let time = now().unwrap();
        let spec = NotebookSpec {
            job_id: JOB.into(),
            node_id: NODE.into(),
            resource_id: "10000000-0000-4000-8000-000000000003".into(),
            owner_id: "10000000-0000-4000-8000-000000000004".into(),
            reservation_id: "10000000-0000-4000-8000-000000000005".into(),
            relay_session_id: SESSION.into(),
            school_id: "synthetic-school".into(),
            hardware_uuid: "GPU-11111111-2222-3333-4444-555555555555".into(),
            not_before: time - 1,
            deadline: time + 60,
            image_digest: IMAGE.into(),
            limits: Limits {
                cpu_millis: 1000,
                memory_bytes: 1073741824,
                pids: 128,
                tmpfs_bytes: 268435456,
            },
        };
        e.authorize_until(JOB, time + 45, time).unwrap();
        e.start(&spec, time).await.unwrap();
        (root, e)
    }
    #[tokio::test]
    async fn actual_cpu_http_uses_only_job_local_target_and_private_upstream_token() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let (root, e) = runtime(port).await;
        let (_, secret) = e.upstream(JOB, now().unwrap()).await.unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut bytes = vec![0; 32768];
            let n = socket.read(&mut bytes).await.unwrap();
            let request = String::from_utf8_lossy(&bytes[..n]);
            assert!(
                request
                    .to_ascii_lowercase()
                    .contains(&format!("authorization: token {secret}"))
            );
            assert!(!request.contains("CBGPU_PRIVATE"));
            assert!(request.contains("_xsrf=fixture"));
            socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 11\r\nSet-Cookie: _xsrf=updated; Domain=evil.invalid; Path=/\r\nSet-Cookie: sb-cbgpu-auth-token=PRIVATE\r\nConnection: close\r\n\r\n{\"ok\":true}").await.unwrap();
        });
        let response = http(
            e.clone(),
            Frame::HttpRequest {
                stream_id: JOB.into(),
                job_id: JOB.into(),
                relay_session_id: SESSION.into(),
                method: "GET".into(),
                uri: format!("/notebooks/{JOB}/api"),
                headers: vec![
                    ("Authorization".into(), "Bearer CBGPU_PRIVATE".into()),
                    (
                        "Cookie".into(),
                        "sb-cbgpu-auth-token=CBGPU_PRIVATE; _xsrf=fixture".into(),
                    ),
                ],
                body: String::new(),
            },
        )
        .await
        .unwrap();
        let Frame::HttpResponse {
            status,
            headers,
            body,
            ..
        } = response
        else {
            panic!("Expected HTTP response")
        };
        assert_eq!(status, 200);
        assert_eq!(
            decode_body(&body, MAX_HTTP_BYTES).unwrap(),
            b"{\"ok\":true}"
        );
        let cookie = headers.iter().find(|(k, _)| k == "set-cookie").unwrap();
        assert_eq!(
            cookie.1,
            format!("_xsrf=updated; Path=/notebooks/{JOB}/; Secure; SameSite=Lax")
        );
        assert!(!format!("{headers:?}").contains("PRIVATE"));
        assert!(!format!("{headers:?}").contains("evil.invalid"));
        server.await.unwrap();
        assert!(
            http(
                e.clone(),
                Frame::HttpRequest {
                    stream_id: JOB.into(),
                    job_id: JOB.into(),
                    relay_session_id: "20000000-0000-4000-8000-000000000006".into(),
                    method: "GET".into(),
                    uri: format!("/notebooks/{JOB}/api"),
                    headers: vec![],
                    body: String::new()
                }
            )
            .await
            .is_err()
        );
        e.stop_all().await.unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }
    #[tokio::test]
    #[allow(clippy::result_large_err)] // tungstenite callback requires its upstream HTTP response error type.
    async fn actual_cpu_websocket_preserves_binary_kernel_frames_and_protocol() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let (root, e) = runtime(port).await;
        let server = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let mut socket=tokio_tungstenite::accept_hdr_async(socket,|request:&tokio_tungstenite::tungstenite::handshake::server::Request,mut response:tokio_tungstenite::tungstenite::handshake::server::Response|{assert!(request.headers()["authorization"].to_str().unwrap().starts_with("token "));assert!(!request.headers().contains_key("cookie"));response.headers_mut().insert("sec-websocket-protocol","v1.kernel.websocket.jupyter.org".parse().unwrap());Ok(response)}).await.unwrap();
            let value = socket.next().await.unwrap().unwrap();
            assert!(matches!(value, Message::Binary(_)));
            socket.send(value).await.unwrap();
            let _ = socket.next().await;
        });
        let (tx, rx) = mpsc::channel(2);
        let (out, mut receive) = mpsc::channel(4);
        let runtime = e.clone();
        let worker = tokio::spawn(async move {
            ws(
                runtime,
                Frame::WsOpen {
                    stream_id: JOB.into(),
                    job_id: JOB.into(),
                    relay_session_id: SESSION.into(),
                    uri: format!("/notebooks/{JOB}/api/kernels/synthetic/channels"),
                    headers: vec![("Authorization".into(), "Bearer CBGPU_PRIVATE".into())],
                    protocols: vec!["v1.kernel.websocket.jupyter.org".into()],
                },
                rx,
                out,
            )
            .await
        });
        assert!(
            matches!(receive.recv().await.unwrap(),Frame::WsOpened{protocol:Some(p),..}if p=="v1.kernel.websocket.jupyter.org")
        );
        let bytes = [0, 255, 17, 0, 128];
        tx.send(Frame::WsData {
            stream_id: JOB.into(),
            binary: true,
            body: encode_body(&bytes),
        })
        .await
        .unwrap();
        let Frame::WsData { binary, body, .. } = receive.recv().await.unwrap() else {
            panic!("Expected binary frame")
        };
        assert!(binary);
        assert_eq!(decode_body(&body, MAX_WS_BYTES).unwrap(), bytes);
        tx.send(Frame::Close {
            stream_id: JOB.into(),
            code: 1000,
        })
        .await
        .unwrap();
        worker.await.unwrap().unwrap();
        server.await.unwrap();
        e.stop_all().await.unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }
}
