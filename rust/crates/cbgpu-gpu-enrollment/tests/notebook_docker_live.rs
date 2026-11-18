//! Explicit disposable CPU fixture only. Never targets an operating notebook or requests a GPU.
#![allow(dead_code)]
#[path = "../src/bin/node_agent/notebook_docker_stream.rs"]
mod notebook_docker_stream;
#[path = "../src/bin/node_agent/notebook_executor.rs"]
mod notebook_executor;
#[path = "../src/bin/node_agent/notebook_gpu_probe.rs"]
mod notebook_gpu_probe;
#[path = "../src/bin/node_agent/notebook_http.rs"]
mod notebook_http;
use futures_util::{SinkExt, StreamExt};
use notebook_executor::{Docker, LocalDocker};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::{Message, client::IntoClientRequest};
#[derive(Deserialize)]
struct Context {
    container_id: String,
    container_name: String,
    job_id: String,
    token: String,
    image_digest: String,
    fixture_run_id: String,
}
async fn api(d: &LocalDocker, c: &Context, method: &str, path: &str, body: Value) -> (u16, Value) {
    let pipe = d.open_stream(&c.container_id).await.unwrap();
    let request = hyper::Request::builder()
        .method(method)
        .uri(format!("/notebooks/{}/{path}", c.job_id))
        .header("Host", "localhost")
        .header("Authorization", format!("token {}", c.token))
        .header("Content-Type", "application/json")
        .body(http_body_util::Full::new(bytes::Bytes::from(
            if body.is_null() {
                vec![]
            } else {
                serde_json::to_vec(&body).unwrap()
            },
        )))
        .unwrap();
    let r = notebook_http::request(pipe, request).await.unwrap();
    let status = r.status().as_u16();
    let bytes = notebook_http::body(r.into_body(), 65536).await.unwrap();
    (
        status,
        if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap()
        },
    )
}
fn uuid() -> String {
    let mut v = [0; 16];
    getrandom::fill(&mut v).unwrap();
    v[6] = (v[6] & 15) | 64;
    v[8] = (v[8] & 63) | 128;
    let h = cbgpu_gpu_enrollment::hex(&v);
    format!(
        "{}-{}-{}-{}-{}",
        &h[..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..]
    )
}
#[tokio::test]
#[ignore = "Requires explicitly owned network-none CPU fixture; no real account, GPU, or production workload"]
async fn actual_docker_hijack_http_and_both_jupyter_kernel_protocols() {
    let path = std::path::PathBuf::from(
        std::env::var_os("CBGPU_NOTEBOOK_CPU_FIXTURE")
            .expect("Explicit private fixture context required"),
    );
    let m = std::fs::symlink_metadata(&path).unwrap();
    assert!(m.is_file());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(m.permissions().mode() & 0o077, 0);
    }
    let c: Context = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert!(
        c.container_name
            .starts_with("cbgpu-notebook-stdio-fixture-")
    );
    assert_eq!(c.container_id.len(), 64);
    let d = LocalDocker::open().unwrap();
    let inspect = d
        .request(
            reqwest::Method::GET,
            &format!("/containers/{}/json", c.container_id),
            None,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(inspect["Name"], format!("/{}", c.container_name));
    assert_eq!(
        inspect["Config"]["Labels"]["io.cbgpu.fixture"],
        "notebook-cpu"
    );
    assert_eq!(
        inspect["Config"]["Labels"]["io.cbgpu.fixture-run"],
        c.fixture_run_id
    );
    assert_eq!(inspect["Config"]["Image"], c.image_digest);
    assert_eq!(inspect["HostConfig"]["NetworkMode"], "none");
    assert_eq!(inspect["HostConfig"]["ReadonlyRootfs"], true);
    assert!(
        inspect["HostConfig"]["DeviceRequests"]
            .as_array()
            .is_none_or(|v| v.is_empty())
    );
    assert!(
        inspect["HostConfig"]["PortBindings"]
            .as_object()
            .is_none_or(|v| v.is_empty())
    );
    assert_eq!(api(&d, &c, "GET", "api/status", Value::Null).await.0, 200);
    let (status, kernel) = api(&d, &c, "POST", "api/kernels", json!({"name":"python3"})).await;
    assert_eq!(status, 201);
    let kernel = kernel["id"].as_str().unwrap();
    for binary in [false, true] {
        let pipe = d.open_stream(&c.container_id).await.unwrap();
        let mut req = format!(
            "ws://localhost/notebooks/{}/api/kernels/{kernel}/channels",
            c.job_id
        )
        .into_client_request()
        .unwrap();
        req.headers_mut().insert(
            "authorization",
            format!("token {}", c.token).parse().unwrap(),
        );
        if binary {
            req.headers_mut().insert(
                "sec-websocket-protocol",
                "v1.kernel.websocket.jupyter.org".parse().unwrap(),
            );
        }
        let (mut socket, response) = tokio_tungstenite::client_async(req, pipe).await.unwrap();
        if binary {
            assert_eq!(
                response.headers()["sec-websocket-protocol"],
                "v1.kernel.websocket.jupyter.org"
            );
        }
        let msg = json!({"header":{"msg_id":uuid(),"username":"synthetic","session":uuid(),"date":"2026-11-18T00:00:00Z","msg_type":"execute_request","version":"5.3"},"parent_header":{},"metadata":{},"content":{"code":"1+1","silent":false,"store_history":false,"user_expressions":{},"allow_stdin":false,"stop_on_error":true},"channel":"shell","buffers":[]});
        if binary {
            let chunks = vec![
                b"shell".to_vec(),
                serde_json::to_vec(&msg["header"]).unwrap(),
                b"{}".to_vec(),
                b"{}".to_vec(),
                serde_json::to_vec(&msg["content"]).unwrap(),
            ];
            let mut offsets = vec![8 * (chunks.len() + 2)];
            for c in &chunks {
                offsets.push(offsets.last().unwrap() + c.len());
            }
            let mut bytes = vec![];
            bytes.extend((offsets.len() as u64).to_le_bytes());
            for o in offsets {
                bytes.extend((o as u64).to_le_bytes());
            }
            for c in chunks {
                bytes.extend(c)
            }
            socket.send(Message::Binary(bytes.into())).await.unwrap();
        } else {
            socket
                .send(Message::Text(msg.to_string().into()))
                .await
                .unwrap();
        }
        let result=tokio::time::timeout(std::time::Duration::from_secs(20),async{for _ in 0..100{let frame=socket.next().await.unwrap().unwrap();let message=match frame{Message::Text(t)=>serde_json::from_str::<Value>(&t).unwrap(),Message::Binary(b)=>{let n=u64::from_le_bytes(b[..8].try_into().unwrap())as usize;assert!((6..=16).contains(&n));let offsets=(0..n).map(|i|u64::from_le_bytes(b[8*(i+1)..8*(i+2)].try_into().unwrap())as usize).collect::<Vec<_>>();json!({"header":serde_json::from_slice::<Value>(&b[offsets[1]..offsets[2]]).unwrap(),"content":serde_json::from_slice::<Value>(&b[offsets[4]..offsets[5]]).unwrap()})},Message::Ping(p)=>{socket.send(Message::Pong(p)).await.unwrap();continue},_=>continue};if message["header"]["msg_type"]=="execute_result"{assert_eq!(message["content"]["data"]["text/plain"],"2");return;}}panic!("Kernel result absent")}).await;
        assert!(result.is_ok());
        let _ = socket.close(None).await;
    }
    assert_eq!(
        api(
            &d,
            &c,
            "DELETE",
            &format!("api/kernels/{kernel}"),
            Value::Null
        )
        .await
        .0,
        204
    );
    std::fs::write(path.parent().unwrap().join("RUST-PASSED.json"),r#"{"dockerExecHTTP":200,"legacyJSON":true,"v1Binary":true,"gpuPassthroughVerified":false,"privateTokensLogged":false}"#).unwrap();
}
