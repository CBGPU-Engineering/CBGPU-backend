#![allow(dead_code)] // Production module also exposes control/relay entry points tested separately.
#[path = "../src/bin/node_agent/notebook_executor.rs"]
mod executor;
use executor as notebook_executor;
#[path = "../src/bin/node_agent/notebook_docker_stream.rs"]
mod notebook_docker_stream;
#[path = "../src/bin/node_agent/notebook_gpu_probe.rs"]
mod notebook_gpu_probe;
#[path = "../src/bin/node_agent/notebook_http.rs"]
mod notebook_http;
use cbgpu_gpu_enrollment::notebooks::protocol::{Limits, NotebookSpec};
use executor::{Docker, Executor};
use serde_json::{Value, json};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};
struct Fake {
    container: Mutex<Option<Value>>,
    calls: Mutex<Vec<String>>,
    lost: Mutex<bool>,
    processes: Mutex<Vec<executor::GpuProcess>>,
}
const IMAGE: &str = "registry.example/approved@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
fn spec() -> NotebookSpec {
    NotebookSpec {
        job_id: "10000000-0000-4000-8000-000000000001".into(),
        node_id: "10000000-0000-4000-8000-000000000002".into(),
        resource_id: "10000000-0000-4000-8000-000000000003".into(),
        owner_id: "10000000-0000-4000-8000-000000000004".into(),
        reservation_id: "10000000-0000-4000-8000-000000000005".into(),
        relay_session_id: "10000000-0000-4000-8000-000000000006".into(),
        school_id: "school-b".into(),
        hardware_uuid: "GPU-11111111-2222-3333-4444-555555555555".into(),
        not_before: 100,
        deadline: 200,
        image_digest: IMAGE.into(),
        limits: Limits {
            cpu_millis: 1000,
            memory_bytes: 1073741824,
            pids: 128,
            tmpfs_bytes: 268435456,
        },
    }
}
impl Docker for Fake {
    async fn notebook_ready(&self, _: &str, _: &str, _: &str) -> executor::Result<bool> {
        Ok(true)
    }
    async fn gpu_processes(&self) -> executor::Result<Vec<executor::GpuProcess>> {
        Ok(self.processes.lock().unwrap().clone())
    }
    async fn request(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
    ) -> executor::Result<Option<Value>> {
        self.calls.lock().unwrap().push(format!("{method} {path}"));
        if path == "/info" {
            return Ok(Some(
                json!({"OSType":"linux","MemTotal":34359738368u64,"NCPU":8,"Runtimes":{"nvidia":{}}}),
            ));
        }
        if path == "/containers/json?all=false" {
            return Ok(Some(json!([])));
        }
        if path.starts_with("/images/") {
            return Ok(Some(json!({"Os":"linux","RepoDigests":[IMAGE]})));
        }
        if path.contains("/create?") {
            let b = body.unwrap();
            assert_eq!(b["HostConfig"]["ReadonlyRootfs"], true);
            assert_eq!(b["HostConfig"]["Privileged"], false);
            assert_eq!(b["HostConfig"]["CapDrop"], json!(["ALL"]));
            assert!(b["HostConfig"].get("Binds").is_none());
            assert_eq!(b["HostConfig"]["LogConfig"]["Config"]["max-size"], "1m");
            assert!(
                b["Cmd"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .all(|a| !a.as_str().unwrap().contains("JUPYTER_TOKEN"))
            );
            let env = b["Env"].as_array().unwrap();
            assert!(env[0].as_str().unwrap().starts_with("JUPYTER_TOKEN="));
            let id = "a".repeat(64);
            *self.container.lock().unwrap() = Some(
                json!({"Id":id,"Config":{"Image":b["Image"],"Labels":b["Labels"],"Env":env},"HostConfig":b["HostConfig"],"Mounts":[{"Type":"bind","Source":b["HostConfig"]["Mounts"][0]["Source"],"Destination":"/usr/local/bin/cbgpu-node-proxy","RW":false}],"State":{"Running":false},"NetworkSettings":{"Ports":{"8888/tcp":[{"HostIp":"127.0.0.1","HostPort":"41234"}]}}}),
            );
            if *self.lost.lock().unwrap() {
                *self.lost.lock().unwrap() = false;
                return Err("DOCKER_UNAVAILABLE");
            }
            return Ok(Some(json!({"Id":id})));
        }
        if path.ends_with("/json") {
            return Ok(self.container.lock().unwrap().clone());
        }
        if path.ends_with("/start") {
            assert!(path.contains(&"a".repeat(64)));
            self.container.lock().unwrap().as_mut().unwrap()["State"]["Running"] = json!(true);
            return Ok(Some(json!({})));
        }
        if path.contains("/stop?") {
            assert!(path.contains(&"a".repeat(64)));
            self.container.lock().unwrap().as_mut().unwrap()["State"]["Running"] = json!(false);
            return Ok(Some(json!({})));
        }
        if method == reqwest::Method::DELETE {
            assert!(path.contains(&"a".repeat(64)));
            *self.container.lock().unwrap() = None;
            return Ok(Some(json!({})));
        }
        panic!("Unexpected fake Docker operation")
    }
}
fn fixture() -> (PathBuf, Arc<Fake>) {
    let mut rand = [0; 8];
    getrandom::fill(&mut rand).unwrap();
    (
        std::env::temp_dir().join(format!(
            "cbgpu-node-executor-{}",
            cbgpu_gpu_enrollment::hex(&rand)
        )),
        Arc::new(Fake {
            container: Mutex::new(None),
            calls: Mutex::new(vec![]),
            lost: Mutex::new(false),
            processes: Mutex::new(vec![]),
        }),
    )
}
#[tokio::test]
async fn sandbox_is_job_bound_idempotent_and_deadline_removes_only_owned_container() {
    let (p, d) = fixture();
    let s = spec();
    let e = Executor::open(d.clone(), &p, &s.node_id, IMAGE)
        .unwrap()
        .with_test_locks();
    e.preflight().await.unwrap();
    assert_eq!(e.start(&s, 150).await.unwrap().state, "RUNNING");
    e.start(&s, 151).await.unwrap();
    assert_eq!(
        d.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|c| c.contains("/create?"))
            .count(),
        1
    );
    let (port, token) = e.upstream(&s.job_id, 151).await.unwrap();
    assert_eq!(port, "a".repeat(64));
    assert_eq!(token.len(), 64);
    assert!(e.upstream(&s.job_id, 200).await.is_err());
    assert_eq!(e.enforce_deadlines(200).await.unwrap()[0].state, "STOPPED");
    assert!(d.container.lock().unwrap().is_none());
    assert!(e.start(&s, 151).await.is_err());
    std::fs::remove_dir_all(p).unwrap();
}
#[tokio::test]
async fn lost_create_ack_recovers_same_generation_without_second_container() {
    let (p, d) = fixture();
    let s = spec();
    *d.lost.lock().unwrap() = true;
    let e = Executor::open(d.clone(), &p, &s.node_id, IMAGE)
        .unwrap()
        .with_test_locks();
    assert!(e.start(&s, 150).await.is_err());
    drop(e); // Simulate the previous process exiting before recovery.
    let e = Executor::open(d.clone(), &p, &s.node_id, IMAGE)
        .unwrap()
        .with_test_locks();
    e.start(&s, 151).await.unwrap();
    assert_eq!(
        d.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|c| c.contains("/create?"))
            .count(),
        1
    );
    std::fs::remove_dir_all(p).unwrap();
}
#[tokio::test]
async fn forged_foreign_labels_or_gpu_never_stop_a_container() {
    let (p, d) = fixture();
    let s = spec();
    let e = Executor::open(d.clone(), &p, &s.node_id, IMAGE)
        .unwrap()
        .with_test_locks();
    e.start(&s, 150).await.unwrap();
    d.container.lock().unwrap().as_mut().unwrap()["Config"]["Labels"]["io.cbgpu.generation"] =
        json!("legacy-gpu1");
    let before = d.calls.lock().unwrap().len();
    assert!(e.stop(&s.job_id).await.is_err());
    assert!(
        d.calls.lock().unwrap()[before..]
            .iter()
            .all(|c| c.starts_with("GET "))
    );
    std::fs::remove_dir_all(p).unwrap();
}
#[tokio::test]
async fn wrong_node_image_deadline_and_port_are_denied() {
    let (p, d) = fixture();
    let mut s = spec();
    let e = Executor::open(d.clone(), &p, &s.node_id, IMAGE)
        .unwrap()
        .with_test_locks();
    s.deadline = 149;
    assert!(e.start(&s, 150).await.is_err());
    s = spec();
    s.node_id = "10000000-0000-4000-8000-000000000099".into();
    assert!(e.start(&s, 150).await.is_err());
    s = spec();
    s.image_digest = "foreign:latest".into();
    assert!(e.start(&s, 150).await.is_err());
    s = spec();
    e.start(&s, 150).await.unwrap();
    d.container.lock().unwrap().as_mut().unwrap()["HostConfig"]["NetworkMode"] = json!("host");
    assert!(e.upstream(&s.job_id, 151).await.is_err());
    std::fs::remove_dir_all(p).unwrap();
}
#[test]
fn local_adapter_is_constructible_without_docker_host_or_host_commands() {
    let _ = executor::LocalDocker::open().unwrap();
}

#[tokio::test]
async fn foreign_gpu_process_blocks_without_any_docker_mutation() {
    let (p, d) = fixture();
    let s = spec();
    d.processes.lock().unwrap().push(executor::GpuProcess {
        uuid: s.hardware_uuid.clone(),
        pid: 424242,
    });
    let e = Executor::open(d.clone(), &p, &s.node_id, IMAGE)
        .unwrap()
        .with_test_locks();
    assert_eq!(e.start(&s, 150).await.unwrap_err(), "GPU_BUSY");
    assert!(
        d.calls
            .lock()
            .unwrap()
            .iter()
            .all(|c| c.starts_with("GET "))
    );
    std::fs::remove_dir_all(p).unwrap();
}
#[tokio::test]
async fn duplicate_agent_cannot_allocate_same_physical_gpu() {
    let (p, d) = fixture();
    let s = spec();
    let shared = p.join("shared-locks");
    let e = Executor::open(d.clone(), &p.join("a"), &s.node_id, IMAGE)
        .unwrap()
        .with_shared_test_locks(&shared);
    e.start(&s, 150).await.unwrap();
    let (_, second) = fixture();
    let mut another = spec();
    another.job_id = "20000000-0000-4000-8000-000000000001".into();
    another.node_id = "20000000-0000-4000-8000-000000000002".into();
    let e2 = Executor::open(second.clone(), &p.join("b"), &another.node_id, IMAGE)
        .unwrap()
        .with_shared_test_locks(&shared);
    assert_eq!(e2.start(&another, 150).await.unwrap_err(), "GPU_BUSY");
    assert!(second.calls.lock().unwrap().is_empty());
    e.stop(&s.job_id).await.unwrap();
    e2.start(&another, 150).await.unwrap();
    std::fs::remove_dir_all(p).unwrap();
}
#[tokio::test]
async fn signed_lease_fence_stops_despite_wall_clock_rollback() {
    let (p, d) = fixture();
    let s = spec();
    let e = Executor::open(d, &p, &s.node_id, IMAGE)
        .unwrap()
        .with_test_locks();
    e.authorize_until(&s.job_id, 151, 150).unwrap();
    e.start(&s, 150).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    assert!(
        e.relay_upstream(&s.job_id, &s.relay_session_id, 149)
            .await
            .is_err()
    );
    assert_eq!(e.enforce_deadlines(149).await.unwrap()[0].state, "STOPPED");
    std::fs::remove_dir_all(p).unwrap();
}

#[tokio::test]
async fn foreign_node_journal_and_stops_are_always_denied() {
    let (p, d) = fixture();
    let s = spec();
    let e = Executor::open(d.clone(), &p, &s.node_id, IMAGE)
        .unwrap()
        .with_test_locks();
    e.start(&s, 150).await.unwrap();
    let other = Executor::open(d.clone(), &p, "20000000-0000-4000-8000-000000000002", IMAGE)
        .unwrap()
        .with_test_locks();
    let before = d.calls.lock().unwrap().len();
    assert_eq!(
        other.stop(&s.job_id).await.unwrap_err(),
        "JOURNAL_BINDING_MISMATCH"
    );
    assert_eq!(
        other.stop_bound(&s).await.unwrap_err(),
        "NOTEBOOK_IDENTITY_CHANGED"
    );
    assert!(other.stop_all().await.is_err());
    assert!(d.calls.lock().unwrap()[before..].is_empty());
    e.stop(&s.job_id).await.unwrap();
    std::fs::remove_dir_all(p).unwrap();
}
