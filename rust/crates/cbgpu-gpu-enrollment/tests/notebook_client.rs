#![allow(dead_code)] // Included production modules expose additional relay/daemon entry points.
#[path = "../src/bin/node_agent/notebook_client.rs"]
mod notebook_client;
#[path = "../src/bin/node_agent/notebook_docker_stream.rs"]
mod notebook_docker_stream;
#[path = "../src/bin/node_agent/notebook_executor.rs"]
mod notebook_executor;
#[path = "../src/bin/node_agent/notebook_gpu_probe.rs"]
mod notebook_gpu_probe;
#[path = "../src/bin/node_agent/notebook_http.rs"]
mod notebook_http;
use cbgpu_gpu_enrollment::{
    hex,
    notebooks::protocol::{self, Envelope, SignedEnvelope, SignedNodeRequest},
};
use ed25519_dalek::SigningKey;
use notebook_client::{Client, Policy, Transport};
use notebook_executor::{Docker, Executor};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
struct FakeDocker;
impl Docker for FakeDocker {
    async fn notebook_ready(&self, _: &str, _: &str, _: &str) -> notebook_executor::Result<bool> {
        Ok(true)
    }
    async fn gpu_processes(&self) -> notebook_executor::Result<Vec<notebook_executor::GpuProcess>> {
        Ok(vec![])
    }
    async fn request(
        &self,
        _: reqwest::Method,
        path: &str,
        _: Option<Value>,
    ) -> notebook_executor::Result<Option<Value>> {
        if path == "/info" {
            Ok(Some(
                json!({"OSType":"linux","MemTotal":34359738368u64,"NCPU":8,"Runtimes":{"nvidia":{}}}),
            ))
        } else if path.starts_with("/images/") {
            Ok(Some(json!({"Os":"linux","RepoDigests":[IMAGE]})))
        } else {
            panic!("No Docker mutations allowed for forged/empty controls")
        }
    }
}
const NODE: &str = "10000000-0000-4000-8000-000000000002";
const IMAGE: &str = "registry.example/image@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
struct Control {
    key: SigningKey,
    requests: Mutex<Vec<SignedNodeRequest>>,
    tamper: bool,
}
impl Transport for Arc<Control> {
    async fn send(
        &self,
        path: &str,
        request: &SignedNodeRequest,
    ) -> notebook_client::Result<SignedEnvelope> {
        assert!(protocol::verify_node_request(
            &request.request,
            &request.signature,
            &hex(SigningKey::from_bytes(&[2; 32]).verifying_key().as_bytes())
        ));
        assert_eq!(request.request.path, path);
        self.requests.lock().unwrap().push(request.clone());
        let now = notebook_client::time().unwrap();
        let mut signed = protocol::sign_envelope(
            Envelope {
                protocol: 3,
                credential_version: 1,
                node_id: NODE.into(),
                request_sequence: request.request.sequence,
                request_method: "POST".into(),
                request_path: path.into(),
                issued_at: now,
                expires_at: now + 60,
                command: None,
            },
            &self.key,
        );
        if self.tamper {
            signed.envelope.request_path.push_str("/forged");
        }
        Ok(signed)
    }
}
fn root() -> std::path::PathBuf {
    let mut v = [0; 8];
    getrandom::fill(&mut v).unwrap();
    std::env::temp_dir().join(format!("cbgpu-notebook-control-{}", hex(&v)))
}
#[tokio::test]
async fn signed_control_persists_sequence_and_rejects_changed_authority_without_docker_launch() {
    for tamper in [false, true] {
        let p = root();
        let key = SigningKey::from_bytes(&[1; 32]);
        let server_key = hex(key.verifying_key().as_bytes());
        let transport = Arc::new(Control {
            key,
            requests: Mutex::new(vec![]),
            tamper,
        });
        let executor = Arc::new(
            Executor::open(Arc::new(FakeDocker), &p, NODE, IMAGE)
                .unwrap()
                .with_test_locks(),
        );
        let mut client = Client::open(
            transport.clone(),
            SigningKey::from_bytes(&[2; 32]),
            Policy {
                protocol: 3,
                enabled: true,
                server_public_key: server_key.clone(),
                image_digest: IMAGE.into(),
            },
            &p.join("control.json"),
            NODE,
            1,
            executor,
        )
        .unwrap();
        let outcome = client.tick().await;
        if tamper {
            assert_eq!(outcome.unwrap_err(), "INVALID_CONTROL_AUTHORITY")
        } else {
            outcome.unwrap();
            client.tick().await.unwrap();
            assert_eq!(transport.requests.lock().unwrap().len(), 2)
        }
        let v: Value =
            serde_json::from_slice(&std::fs::read(p.join("control.json")).unwrap()).unwrap();
        assert!(v["sequence"].as_i64().unwrap() > 0);
        assert!(v.get("private_key").is_none());
        std::fs::remove_dir_all(p).unwrap();
    }
}
#[test]
fn registry_transport_forbids_plain_http_userinfo_paths_and_untrusted_custom_port() {
    for value in [
        "http://registry.example",
        "https://user:password@registry.example",
        "https://registry.example/api",
        "https://registry.example:444/",
    ] {
        assert!(notebook_client::Http::open(value).is_err())
    }
}
