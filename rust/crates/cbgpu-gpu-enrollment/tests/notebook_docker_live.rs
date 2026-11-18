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
async fn verify_kernel_protocols(d: &LocalDocker, c: &Context) {
    assert_eq!(api(d, c, "GET", "api/status", Value::Null).await.0, 200);
    let (status, kernel) = api(d, c, "POST", "api/kernels", json!({"name":"python3"})).await;
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
            d,
            c,
            "DELETE",
            &format!("api/kernels/{kernel}"),
            Value::Null
        )
        .await
        .0,
        204
    );
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
    verify_kernel_protocols(&d, &c).await;
    std::fs::write(path.parent().unwrap().join("RUST-PASSED.json"),r#"{"dockerExecHTTP":200,"legacyJSON":true,"v1Binary":true,"gpuPassthroughVerified":false,"privateTokensLogged":false}"#).unwrap();
}
#[tokio::test]
#[ignore = "Creates only a new bounded CPU fixture from the production sandbox spec; requires approved static ELF"]
async fn production_sandbox_env_token_and_startup_are_verified_on_pinned_image() {
    use sha2::{Digest, Sha256};
    use std::{io::Write, path::PathBuf};
    let binary = PathBuf::from(
        std::env::var_os("CBGPU_NOTEBOOK_STATIC_ELF")
            .expect("Explicit verified static ELF required"),
    );
    let expected =
        std::env::var("CBGPU_NOTEBOOK_STATIC_SHA256").expect("Explicit ELF SHA required");
    let bytes = std::fs::read(&binary).unwrap();
    assert_eq!(format!("{:x}", Sha256::digest(&bytes)), expected);
    assert!(bytes.starts_with(b"\x7fELF"));
    let job = uuid();
    let root = std::env::temp_dir().join(format!("cbgpu-notebook-sandbox-cpu-{job}"));
    let mut dir = std::fs::DirBuilder::new();
    dir.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        dir.mode(0o700);
    }
    dir.create(&root).unwrap();
    let proxy = root.join(format!("{expected}.elf"));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    let mut f = options.open(&proxy).unwrap();
    f.write_all(&bytes).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        f.set_permissions(std::fs::Permissions::from_mode(0o555))
            .unwrap();
    }
    drop(f);
    let token = uuid().replace('-', "") + &uuid().replace('-', "");
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let image = "quay.io/jupyter/pytorch-notebook@sha256:83066a1f1108f35cfea7b0f93ae24bb70d6e55c8eeff82b4f9a86dae4496f77b";
    let spec = cbgpu_gpu_enrollment::notebooks::protocol::NotebookSpec {
        job_id: job.clone(),
        node_id: uuid(),
        resource_id: uuid(),
        reservation_id: uuid(),
        owner_id: uuid(),
        school_id: "synthetic-cpu-only".into(),
        hardware_uuid: "GPU-11111111-2222-3333-4444-555555555555".into(),
        relay_session_id: uuid(),
        not_before: now - 1,
        deadline: now + 120,
        image_digest: image.into(),
        limits: cbgpu_gpu_enrollment::notebooks::protocol::Limits {
            cpu_millis: 1000,
            memory_bytes: 2147483648,
            pids: 256,
            tmpfs_bytes: 1073741824,
        },
    };
    let mut body = notebook_executor::sandbox_for_cpu_fixture(&spec, &token, now, &proxy);
    assert_eq!(body["HostConfig"]["NetworkMode"], "none");
    assert_eq!(body["HostConfig"]["ReadonlyRootfs"], true);
    assert!(body["HostConfig"].get("PortBindings").is_none());
    assert_eq!(body["HostConfig"]["Mounts"].as_array().unwrap().len(), 1);
    // This explicit CPU fixture cannot request a GPU or activate the NVIDIA runtime.
    body["HostConfig"]["DeviceRequests"] = json!([]);
    body["HostConfig"]["Runtime"] = json!("runc");
    body["Env"]
        .as_array_mut()
        .unwrap()
        .push(json!("NVIDIA_VISIBLE_DEVICES=void"));
    body["Labels"]["io.cbgpu.fixture"] = json!("production-sandbox-cpu");
    let name = format!("cbgpu-notebook-sandbox-fixture-{job}");
    let d = LocalDocker::open().unwrap();
    let created = d
        .request(
            reqwest::Method::POST,
            &format!("/containers/create?name={name}"),
            Some(body),
        )
        .await
        .unwrap()
        .unwrap();
    let id = created["Id"].as_str().unwrap().to_owned();
    assert_eq!(id.len(), 64);
    struct OwnCpu(String);
    impl Drop for OwnCpu {
        fn drop(&mut self) {
            let _ = std::process::Command::new("docker")
                .args(["rm", "-f", &self.0])
                .output();
        }
    }
    let inspected = d
        .request(
            reqwest::Method::GET,
            &format!("/containers/{id}/json"),
            None,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        inspected["Config"]["Labels"]["io.cbgpu.fixture"],
        "production-sandbox-cpu"
    );
    assert_eq!(inspected["Name"], format!("/{name}"));
    assert_eq!(inspected["HostConfig"]["NetworkMode"], "none");
    assert_eq!(inspected["HostConfig"]["Runtime"], "runc");
    assert!(
        inspected["HostConfig"]["DeviceRequests"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let guard = OwnCpu(id.clone());
    if d.request(
        reqwest::Method::POST,
        &format!("/containers/{id}/start"),
        None,
    )
    .await
    .is_err()
    {
        let raw = reqwest::Client::builder()
            .unix_socket("/var/run/docker.sock")
            .no_proxy()
            .build()
            .unwrap()
            .post(format!("http://localhost/v1.43/containers/{id}/start"))
            .send()
            .await
            .unwrap();
        let bytes = raw.bytes().await.unwrap();
        std::fs::write(root.join("start-error.private"), bytes).unwrap();
        panic!(
            "Owned CPU fixture start failed; private diagnostic {}",
            root.display()
        );
    }
    let mut ready = false;
    for _ in 0..30 {
        if d.notebook_ready(&id, &token, &job).await.unwrap_or(false) {
            ready = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    assert!(
        ready,
        "Production sandbox Jupyter startup must pass real token health before advertising RUNNING"
    );
    let c = Context {
        container_id: id.clone(),
        container_name: name,
        job_id: job.clone(),
        token,
        image_digest: image.into(),
        fixture_run_id: job,
    };
    verify_kernel_protocols(&d, &c).await;
    let report = json!({"kernel_json":true,"kernel_v1_binary":true,"source_sandbox_used":true,"network":"none","gpu_requests":0,"published_ports":0,"readonly_root":true,"private_env_token":true,"jupyter_health":200,"physical_gpu_verified":false,"elf_sha256":expected});
    std::fs::write(root.join("PASSED.json"), report.to_string()).unwrap();
    println!("Bounded production-spec CPU startup PASSED; no physical GPU was requested.");
    drop(guard);
}
