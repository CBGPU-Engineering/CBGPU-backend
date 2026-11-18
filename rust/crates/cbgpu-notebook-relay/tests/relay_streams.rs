//! Actual loopback TCP/WebSocket transport; fake authorization and no hardware/provider calls.
use async_trait::async_trait;
use cbgpu_notebook_protocol::{Frame, decode_body, decode_frame, encode_body, encode_frame};
use cbgpu_notebook_relay::{
    Error, Gateway, Result,
    authority::{Authority, Grant, Identity},
    router,
};
use ed25519_dalek::SigningKey;
use futures_util::{SinkExt, StreamExt};
use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicI64, Ordering},
    },
    time::Duration,
};
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream, connect_async,
    tungstenite::{Message, client::IntoClientRequest},
};
const NODE: &str = "11111111-1111-4111-8111-111111111111";
const JOB: &str = "22222222-2222-4222-8222-222222222222";
const OTHER: &str = "33333333-3333-4333-8333-333333333333";
const TOKEN: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const HOST: &str = "notebooks.cbgpu.com";
type Socket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;
struct Fake {
    allowed: AtomicBool,
    version: AtomicI64,
    connections: Mutex<HashMap<String, bool>>,
    jobs: HashMap<String, String>,
}
#[async_trait]
impl Authority for Fake {
    async fn identity(&self, _node: &str) -> Result<Identity> {
        Ok(Identity {
            key: SigningKey::from_bytes(&[4; 32]).verifying_key(),
            version: self.version.load(Ordering::SeqCst),
        })
    }
    async fn grant(&self, token: &str, job: &str) -> Result<Grant> {
        if !self.allowed.load(Ordering::SeqCst) || token != TOKEN {
            return Err(Error::Denied);
        }
        let node = self.jobs.get(job).ok_or(Error::Denied)?;
        Ok(Grant {
            job_id: job.into(),
            node_id: node.clone(),
            version: self.version.load(Ordering::SeqCst),
            relay_session_id: "44444444-4444-4444-8444-444444444444".into(),
            expires_at: cbgpu_notebook_relay::now() + 3600,
        })
    }
    async fn relay(&self, node: &str, _version: i64, connected: bool) -> Result<()> {
        self.connections
            .lock()
            .unwrap()
            .insert(node.into(), connected);
        Ok(())
    }
}
async fn server(jobs: HashMap<String, String>) -> (String, Arc<Fake>, tokio::task::JoinHandle<()>) {
    let auth = Arc::new(Fake {
        allowed: AtomicBool::new(true),
        version: AtomicI64::new(1),
        connections: Mutex::new(HashMap::new()),
        jobs,
    });
    let mut gateway = Gateway::new(auth.clone(), SigningKey::from_bytes(&[5; 32]));
    Arc::get_mut(&mut gateway).unwrap().recheck = Duration::from_millis(30);
    Arc::get_mut(&mut gateway).unwrap().bootstrap_key =
        "synthetic-private-bootstrap-key-000000".into();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let task = tokio::spawn(async move { axum::serve(listener, router(gateway)).await.unwrap() });
    (addr.to_string(), auth, task)
}
async fn node(addr: &str, node: &str, version: i64) -> tokio::task::JoinHandle<()> {
    let mut request = format!("ws://{addr}/v1/gpu/relay")
        .into_client_request()
        .unwrap();
    request
        .headers_mut()
        .insert("x-cbgpu-node-id", node.parse().unwrap());
    request.headers_mut().insert(
        "x-cbgpu-credential-version",
        version.to_string().parse().unwrap(),
    );
    let (mut socket, _) = tokio::time::timeout(Duration::from_secs(3), connect_async(request))
        .await
        .expect("WS HTTP101 upgrade must complete")
        .unwrap();
    let Message::Text(text) = tokio::time::timeout(Duration::from_secs(3), socket.next())
        .await
        .expect("signed challenge must flow after WS101")
        .unwrap()
        .unwrap()
    else {
        panic!("challenge")
    };
    let Frame::Challenge { challenge } = decode_frame(&text).unwrap() else {
        panic!("challenge")
    };
    assert!(challenge.verify_server(
        &SigningKey::from_bytes(&[5; 32]).verifying_key(),
        cbgpu_notebook_relay::now()
    ));
    socket
        .send(Message::Text(
            encode_frame(&Frame::Auth {
                signature: challenge.node_signature(&SigningKey::from_bytes(&[4; 32])),
            })
            .unwrap()
            .into(),
        ))
        .await
        .unwrap();
    let Message::Text(text) = socket.next().await.unwrap().unwrap() else {
        panic!("ready")
    };
    assert!(matches!(decode_frame(&text).unwrap(), Frame::Ready {}));
    tokio::spawn(async move {
        while let Some(Ok(Message::Text(text))) = socket.next().await {
            let frame = decode_frame(&text).unwrap();
            let reply = match frame {
                Frame::HttpRequest {
                    stream_id,
                    job_id,
                    headers,
                    ..
                } => {
                    assert!(
                        !headers
                            .iter()
                            .any(|(k, _)| k.eq_ignore_ascii_case("authorization"))
                    );
                    Frame::HttpResponse {
                        stream_id,
                        status: 200,
                        headers: vec![("content-type".into(), "text/plain".into())],
                        body: encode_body(job_id.as_bytes()),
                    }
                }
                Frame::WsOpen { stream_id, .. } => Frame::WsOpened {
                    stream_id,
                    protocol: None,
                },
                Frame::WsData {
                    stream_id,
                    binary,
                    body,
                } => Frame::WsData {
                    stream_id,
                    binary,
                    body,
                },
                Frame::Ping {} => Frame::Pong {},
                Frame::Close { .. } => continue,
                Frame::Revoked {} => break,
                _ => panic!("unexpected relay frame"),
            };
            if socket
                .send(Message::Text(encode_frame(&reply).unwrap().into()))
                .await
                .is_err()
            {
                break;
            }
        }
    })
}
async fn browser(
    addr: &str,
    job: &str,
) -> std::result::Result<Socket, tokio_tungstenite::tungstenite::Error> {
    let mut request = format!("ws://{addr}/notebooks/{job}/api/kernels/fake/channels")
        .into_client_request()
        .unwrap();
    for (k, v) in [
        ("host", HOST),
        ("origin", "https://notebooks.cbgpu.com"),
        ("cookie", &format!("__Secure-cbgpu-notebook={TOKEN}")),
    ] {
        request.headers_mut().insert(
            axum::http::HeaderName::from_bytes(k.as_bytes()).unwrap(),
            v.parse().unwrap(),
        );
    }
    connect_async(request).await.map(|v| v.0)
}
fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(3))
        .build()
        .unwrap()
}
#[tokio::test]
async fn actual_signed_node_http_ws_cross_job_revocation_and_rekey() {
    let (addr, auth, server) = server(HashMap::from([(JOB.into(), NODE.into())])).await;
    let old = node(&addr, NODE, 1).await;
    let response = client()
        .get(format!("http://{addr}/notebooks/{JOB}/lab"))
        .header("host", HOST)
        .header("cookie", format!("__Secure-cbgpu-notebook={TOKEN}"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(response.text().await.unwrap(), JOB);
    let denied = client()
        .get(format!("http://{addr}/notebooks/{OTHER}/lab"))
        .header("host", HOST)
        .header("cookie", format!("__Secure-cbgpu-notebook={TOKEN}"))
        .send()
        .await
        .unwrap();
    assert_eq!(denied.status(), 403);
    let mut ws = browser(&addr, JOB).await.unwrap();
    ws.send(Message::Text("actual tcp echo".into()))
        .await
        .unwrap();
    let msg = tokio::time::timeout(Duration::from_secs(2), ws.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(msg.into_text().unwrap(), "actual tcp echo");
    auth.allowed.store(false, Ordering::SeqCst);
    let closed = tokio::time::timeout(Duration::from_secs(2), ws.next())
        .await
        .unwrap();
    assert!(
        closed.is_none()
            || closed.is_some_and(|m| m.is_err() || matches!(m, Ok(Message::Close(_))))
    );
    auth.allowed.store(true, Ordering::SeqCst);
    let mut before = browser(&addr, JOB).await.unwrap();
    auth.version.store(2, Ordering::SeqCst);
    let replacement = node(&addr, NODE, 2).await;
    let closed = tokio::time::timeout(Duration::from_secs(2), before.next())
        .await
        .unwrap();
    assert!(
        closed.is_none()
            || closed.is_some_and(|m| m.is_err() || matches!(m, Ok(Message::Close(_))))
    );
    assert!(auth.connections.lock().unwrap().get(NODE) == Some(&true));
    assert!(browser(&addr, JOB).await.is_ok());
    old.abort();
    replacement.abort();
    server.abort();
}
#[tokio::test]
async fn failed_upgrade_releases_slots_and_per_job_eighth_limit_is_enforced() {
    let (addr, _auth, server) = server(HashMap::from([(JOB.into(), NODE.into())])).await;
    let peer = node(&addr, NODE, 1).await;
    // Each malformed upgrade fails after stream allocation; none may leak the eight job slots.
    for _ in 0..12 {
        let response = client()
            .get(format!("http://{addr}/notebooks/{JOB}/lab"))
            .header("host", HOST)
            .header("origin", "https://notebooks.cbgpu.com")
            .header("cookie", format!("__Secure-cbgpu-notebook={TOKEN}"))
            .header("upgrade", "websocket")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 400);
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let mut sockets = Vec::new();
    for _ in 0..8 {
        sockets.push(browser(&addr, JOB).await.unwrap());
    }
    let denied = browser(&addr, JOB).await.unwrap_err();
    let tokio_tungstenite::tungstenite::Error::Http(response) = denied else {
        panic!("must reject ninth stream")
    };
    assert_eq!(response.status(), 429);
    let mut released = sockets.pop().unwrap();
    released.close(None).await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(browser(&addr, JOB).await.is_ok());
    peer.abort();
    server.abort();
}
#[test]
fn frame_body_roundtrip() {
    assert_eq!(decode_body(&encode_body(b"ok"), 2).unwrap(), b"ok");
}

#[tokio::test]
async fn node_32_and_global_64_stream_budgets_are_real_transport_limits() {
    let nodes = [
        NODE,
        "55555555-5555-4555-8555-555555555555",
        "66666666-6666-4666-8666-666666666666",
    ];
    let jobs = (1..=10)
        .map(|i| format!("77777777-7777-4777-8777-{i:012}"))
        .collect::<Vec<_>>();
    let mapping = jobs
        .iter()
        .enumerate()
        .map(|(i, j)| {
            (
                j.clone(),
                nodes[if i < 5 {
                    0
                } else if i < 9 {
                    1
                } else {
                    2
                }]
                .into(),
            )
        })
        .collect();
    let (addr, _auth, server) = server(mapping).await;
    let mut peers = Vec::new();
    for node_id in nodes {
        peers.push(node(&addr, node_id, 1).await);
    }
    let mut sockets = Vec::new();
    for job in jobs.iter().take(4) {
        for _ in 0..8 {
            sockets.push(browser(&addr, job).await.unwrap());
        }
    }
    // Another job on this node bypasses per-job saturation but must hit the node budget.
    let tokio_tungstenite::tungstenite::Error::Http(response) =
        browser(&addr, &jobs[4]).await.unwrap_err()
    else {
        panic!("node budget")
    };
    assert_eq!(response.status(), 429);
    for job in jobs.iter().skip(5).take(4) {
        for _ in 0..8 {
            sockets.push(browser(&addr, job).await.unwrap());
        }
    }
    let tokio_tungstenite::tungstenite::Error::Http(response) =
        browser(&addr, &jobs[9]).await.unwrap_err()
    else {
        panic!("global budget")
    };
    assert_eq!(response.status(), 429);
    drop(sockets);
    for peer in peers {
        peer.abort();
    }
    server.abort();
}

#[tokio::test]
async fn bootstrap_exchange_is_private_one_use_job_origin_and_host_bound() {
    let (addr, auth, server) = server(HashMap::from([(JOB.into(), NODE.into())])).await;
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(3))
        .build()
        .unwrap();
    let mint_url = format!("http://{addr}/internal/bootstrap");
    let input =
        serde_json::json!({"job_id":JOB,"cookie_token":TOKEN,"parent_origin":"https://cbgpu.com"});
    for key in [None, Some("wrong-private-key")] {
        let mut r = client.post(&mint_url).json(&input);
        if let Some(key) = key {
            r = r.bearer_auth(key);
        }
        let response = r.send().await.unwrap();
        assert_eq!(response.status(), 403);
        assert!(!response.text().await.unwrap().contains(TOKEN));
    }
    let response = client
        .post(&mint_url)
        .bearer_auth("synthetic-private-bootstrap-key-000000")
        .json(&input)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(response.headers()["cache-control"], "private, no-store");
    let raw = response.text().await.unwrap();
    assert!(!raw.contains(TOKEN));
    let minted: serde_json::Value = serde_json::from_str(&raw).unwrap();
    let code = minted["bootstrap_token"].as_str().unwrap();
    assert_eq!(code.len(), 64);
    assert!(code.bytes().all(|b| b.is_ascii_hexdigit()));
    assert_eq!(minted["notebook_origin"], "https://notebooks.cbgpu.com");
    let form = format!("bootstrap_token={code}");
    for (job, origin, host) in [
        (OTHER, "https://cbgpu.com", HOST),
        (JOB, "https://app.cbgpu.com", HOST),
        (JOB, "https://attacker.invalid", HOST),
        (JOB, "https://cbgpu.com", "cbgpu.com"),
    ] {
        let response = client
            .post(format!("http://{addr}/notebooks/{job}/_session"))
            .header("host", host)
            .header("origin", origin)
            .header("content-type", "application/x-www-form-urlencoded")
            .body(form.clone())
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 403);
        assert!(response.headers().get("set-cookie").is_none());
        let body = response.text().await.unwrap();
        assert!(!body.contains(TOKEN));
        assert!(!body.contains(code));
    }
    let redeem = format!("http://{addr}/notebooks/{JOB}/_session");
    let response = client
        .post(&redeem)
        .header("host", HOST)
        .header("origin", "https://cbgpu.com")
        .header("content-type", "application/x-www-form-urlencoded")
        .body(form.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 303);
    assert_eq!(
        response.headers()["location"],
        format!("/notebooks/{JOB}/lab")
    );
    let cookie = response.headers()["set-cookie"].to_str().unwrap();
    assert!(cookie.starts_with(&format!("__Secure-cbgpu-notebook={TOKEN};")));
    assert!(cookie.contains(&format!("Path=/notebooks/{JOB}/;")));
    for expected in ["Secure", "HttpOnly", "SameSite=Lax", "Max-Age="] {
        assert!(cookie.contains(expected));
    }
    assert!(!cookie.to_ascii_lowercase().contains("domain="));
    assert!(
        !response.headers()["location"]
            .to_str()
            .unwrap()
            .contains(code)
    );
    assert!(response.text().await.unwrap().is_empty());
    let replay = client
        .post(&redeem)
        .header("host", HOST)
        .header("origin", "https://cbgpu.com")
        .header("content-type", "application/x-www-form-urlencoded")
        .body(form)
        .send()
        .await
        .unwrap();
    assert_eq!(replay.status(), 403);
    assert!(replay.headers().get("set-cookie").is_none());
    let response = client
        .get(format!("http://{addr}/notebooks/{JOB}/lab"))
        .header("host", "cbgpu.com")
        .header("cookie", format!("__Secure-cbgpu-notebook={TOKEN}"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 403);
    let response = client
        .post(format!("http://{addr}/internal/unknown-bootstrap"))
        .bearer_auth("synthetic-private-bootstrap-key-000000")
        .json(&input)
        .send()
        .await
        .unwrap();
    assert!(!response.status().is_success());
    assert!(!response.text().await.unwrap().contains(TOKEN));
    // A fresh authority denial between mint and redeem cannot issue a cookie.
    let response = client
        .post(&mint_url)
        .bearer_auth("synthetic-private-bootstrap-key-000000")
        .json(&input)
        .send()
        .await
        .unwrap();
    let minted: serde_json::Value = response.json().await.unwrap();
    auth.allowed.store(false, Ordering::SeqCst);
    let response = client
        .post(&redeem)
        .header("host", HOST)
        .header("origin", "https://cbgpu.com")
        .header("content-type", "application/x-www-form-urlencoded")
        .body(format!(
            "bootstrap_token={}",
            minted["bootstrap_token"].as_str().unwrap()
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 403);
    assert!(response.headers().get("set-cookie").is_none());
    server.abort();
}

#[tokio::test]
#[ignore = "Explicit immutable Next artifact; fake authority, own loopback13845 and no production env files"]
async fn actual_next16_rewrites_authenticated_node_and_notebook_websockets() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let artifact = std::path::PathBuf::from(
        std::env::var("CBGPU_NEXT_ARTIFACT").expect("explicit reviewed artifact path"),
    );
    let build = if artifact.join(".next/BUILD_ID").is_file() {
        artifact.join(".next")
    } else {
        artifact.join(".next-production")
    };
    assert!(build.join("BUILD_ID").is_file());
    let temp =
        std::env::temp_dir().join(format!("cbgpu-next-relay-proof-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&temp).unwrap();
    std::fs::set_permissions(&temp, std::fs::Permissions::from_mode(0o700)).unwrap();
    for file in ["next.config.ts", "package.json"] {
        std::fs::copy(artifact.join(file), temp.join(file)).unwrap();
    }
    symlink(&build, temp.join(".next")).unwrap();
    symlink(artifact.join("node_modules"), temp.join("node_modules")).unwrap();
    let edge = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    let key = temp.join("edge-key");
    std::fs::write(&key, edge).unwrap();
    std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o600)).unwrap();
    let (addr, _auth, server) = server(HashMap::from([(JOB.into(), NODE.into())])).await;
    let port = 13845;
    {
        let _available =
            std::net::TcpListener::bind(("127.0.0.1", port)).expect("fixture port must be free");
    }
    let log = std::fs::File::create(temp.join("next.log")).unwrap();
    std::fs::set_permissions(
        temp.join("next.log"),
        std::fs::Permissions::from_mode(0o600),
    )
    .unwrap();
    struct Next(std::process::Child);
    impl Drop for Next {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let source = std::env::var("CBGPU_NEXT_SERVER_SOURCE")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| artifact.clone());
    let custom = source.join("scripts/serve.mts").is_file();
    let mut command = std::process::Command::new("node");
    if custom {
        for file in [
            "scripts/serve.mts",
            "src/lib/notebookUpgrade.ts",
            "src/lib/notebookEdge.ts",
            "src/lib/internalServiceUrl.ts",
        ] {
            let destination = temp.join(file);
            std::fs::create_dir_all(destination.parent().unwrap()).unwrap();
            std::fs::copy(source.join(file), destination).unwrap();
        }
        command.arg(temp.join("scripts/serve.mts"));
    } else {
        command
            .arg(artifact.join("node_modules/next/dist/bin/next"))
            .args([
                "start",
                "--hostname",
                "127.0.0.1",
                "--port",
                &port.to_string(),
            ]);
    }
    let child = command
        .current_dir(&temp)
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .env("HOME", &temp)
        .env("NODE_ENV", "production")
        .env("HOSTNAME", "127.0.0.1")
        .env("PORT", port.to_string())
        .env("NEXT_TELEMETRY_DISABLED", "1")
        .env("CBGPU_GPU_NOTEBOOKS_ENABLED", "1")
        .env("CBGPU_NOTEBOOK_RELAY_URL", format!("http://{addr}"))
        .env("CBGPU_NOTEBOOK_EDGE_KEY_FILE", &key)
        .stdout(log.try_clone().unwrap())
        .stderr(log)
        .spawn()
        .unwrap();
    let _next = Next(child);
    let next_addr = format!("127.0.0.1:{port}");
    let mut ready = false;
    for _ in 0..100 {
        if let Ok(r) = client()
            .get(format!("http://{next_addr}/v1/gpu/relay"))
            .header("host", "cbgpu.com")
            .send()
            .await
            && r.status().as_u16() == 400
        {
            ready = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(
        ready,
        "Next rewrite startup failure; private test log retained"
    );
    let peer = tokio::time::timeout(Duration::from_secs(10), node(&next_addr, NODE, 1))
        .await
        .expect("Next must forward node WS upgrade");
    let request_url = format!("http://{next_addr}/__notebook_gateway/notebooks/{JOB}/lab");
    let denied = client()
        .get(&request_url)
        .header("host", "cbgpu.com")
        .header("cookie", format!("__Secure-cbgpu-notebook={TOKEN}"))
        .send()
        .await
        .unwrap();
    assert_eq!(denied.status(), 404);
    let response = client()
        .get(&request_url)
        .header("host", "cbgpu.com")
        .header("origin", "https://notebooks.cbgpu.com")
        .header("x-cbgpu-note-edge", edge)
        .header("cookie", format!("__Secure-cbgpu-notebook={TOKEN}"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(response.text().await.unwrap(), JOB);
    let mut request =
        format!("ws://{next_addr}/__notebook_gateway/notebooks/{JOB}/api/kernels/fake/channels")
            .into_client_request()
            .unwrap();
    for (k, v) in [
        ("host", "cbgpu.com"),
        ("origin", "https://notebooks.cbgpu.com"),
        ("x-cbgpu-note-edge", edge),
        ("cookie", &format!("__Secure-cbgpu-notebook={TOKEN}")),
    ] {
        request.headers_mut().insert(
            axum::http::HeaderName::from_bytes(k.as_bytes()).unwrap(),
            v.parse().unwrap(),
        );
    }
    let (mut browser, _) = tokio::time::timeout(Duration::from_secs(10), connect_async(request))
        .await
        .expect("Next must forward notebook WS upgrade")
        .unwrap();
    browser
        .send(Message::Text("through actual Next16".into()))
        .await
        .unwrap();
    let reply = tokio::time::timeout(Duration::from_secs(5), browser.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(reply.into_text().unwrap(), "through actual Next16");
    browser.close(None).await.unwrap();
    peer.abort();
    server.abort();
    std::fs::write(temp.join("PASSED.json"),serde_json::json!({"artifact":artifact,"custom_server_source":source,"actual_next_node_ws":true,"actual_next_notebook_http_ws":true,"synthetic_edge_key_only":true,"production_env_files_read":false,"production_services_changed":false}).to_string()).unwrap();
    println!("Next relay proof receipt: {}", temp.display());
}
