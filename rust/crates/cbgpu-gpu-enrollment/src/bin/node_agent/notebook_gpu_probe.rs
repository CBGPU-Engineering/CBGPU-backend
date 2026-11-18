//! Fixed read-only NVIDIA passthrough probe inside a new, owned, network-none sandbox.
use super::notebook_executor::{Docker, Error, Result};
use cbgpu_gpu_enrollment::valid_hardware_uuid;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
fn labels(node: &str, gpu: &str, name: &str) -> Value {
    json!({"io.cbgpu.managed":"gpu-preflight-v3","io.cbgpu.node":node,"io.cbgpu.gpu":gpu,"io.cbgpu.probe":name})
}
fn owned(v: &Value, node: &str, gpu: &str, name: &str, image: &str) -> bool {
    v["Id"]
        .as_str()
        .is_some_and(|v| v.len() == 64 && v.bytes().all(|b| b.is_ascii_hexdigit()))
        && v["Config"]["Image"] == image
        && labels(node, gpu, name).as_object().is_some_and(|l| {
            l.iter()
                .all(|(k, val)| v["Config"]["Labels"].get(k) == Some(val))
        })
        && v["HostConfig"]["NetworkMode"] == "none"
        && v["HostConfig"]["Privileged"] == false
        && v["HostConfig"]["ReadonlyRootfs"] == true
        && v["HostConfig"]["DeviceRequests"]
            .as_array()
            .is_some_and(|r| r.len() == 1 && r[0]["DeviceIDs"] == json!([gpu]))
}
fn stdout(bytes: &[u8]) -> Result<Vec<u8>> {
    let mut remaining = bytes;
    let mut out = vec![];
    while !remaining.is_empty() {
        if remaining.len() < 8 || remaining[1..4] != [0, 0, 0] || ![1, 2].contains(&remaining[0]) {
            return Err("GPU_PROBE_FAILED");
        }
        let n = u32::from_be_bytes(remaining[4..8].try_into().map_err(|_| "GPU_PROBE_FAILED")?)
            as usize;
        if n > 16384 || remaining.len() < 8 + n {
            return Err("GPU_PROBE_FAILED");
        }
        if remaining[0] == 1 {
            out.extend_from_slice(&remaining[8..8 + n]);
            if out.len() > 16384 {
                return Err("GPU_PROBE_FAILED");
            }
        }
        remaining = &remaining[8 + n..];
    }
    Ok(out)
}
pub async fn run(docker: &impl Docker, node: &str, image: &str, gpu: &str) -> Result<()> {
    if !valid_hardware_uuid(gpu) {
        return Err("GPU_PROBE_FAILED");
    }
    let digest = format!("{:x}", Sha256::digest(format!("{node}:{gpu}:{image}")));
    let name = format!("cbgpu-node-probe-{}", &digest[..32]);
    let path = format!("/containers/{name}/json");
    let mut current = docker.request(reqwest::Method::GET, &path, None).await?;
    if current.is_none() {
        let body = json!({"Image":image,"Labels":labels(node,gpu,&name),"User":"1000:100","Entrypoint":["/usr/bin/timeout"],"Cmd":["--signal=TERM","--kill-after=2","10","/usr/bin/nvidia-smi","--query-gpu=uuid","--format=csv,noheader"],"Env":[format!("NVIDIA_VISIBLE_DEVICES={gpu}"),"NVIDIA_DRIVER_CAPABILITIES=utility","NVIDIA_REQUIRE_CUDA=cuda>=12.8","NVIDIA_DISABLE_REQUIRE=false"],"HostConfig":{"NetworkMode":"none","ReadonlyRootfs":true,"Privileged":false,"CapDrop":["ALL"],"SecurityOpt":["no-new-privileges:true"],"Memory":268435456,"MemorySwap":268435456,"NanoCpus":500000000u64,"PidsLimit":32,"Tmpfs":{"/tmp":"rw,nosuid,nodev,size=16777216"},"DeviceRequests":[{"Driver":"nvidia","DeviceIDs":[gpu],"Capabilities":[["gpu"]]}],"RestartPolicy":{"Name":"no"},"LogConfig":{"Type":"local","Config":{"max-size":"1m","max-file":"1","compress":"false"}}}});
        let created = docker
            .request(
                reqwest::Method::POST,
                &format!("/containers/create?name={name}"),
                Some(body),
            )
            .await;
        current = docker.request(reqwest::Method::GET, &path, None).await?;
        if current.is_none() {
            created?;
            return Err("GPU_PROBE_FAILED");
        }
    }
    let current = current.ok_or("GPU_PROBE_FAILED")?;
    if !owned(&current, node, gpu, &name, image) {
        return Err("CONTAINER_OWNERSHIP_MISMATCH");
    }
    let id = current["Id"].as_str().ok_or("GPU_PROBE_FAILED")?.to_owned();
    let outcome = async {
        if current["State"]["Running"] != true && current["State"]["Status"] != "exited" {
            docker
                .request(
                    reqwest::Method::POST,
                    &format!("/containers/{id}/start"),
                    None,
                )
                .await?
                .ok_or("GPU_PROBE_FAILED")?;
        }
        let wait = tokio::time::timeout(
            std::time::Duration::from_secs(15),
            docker.request(
                reqwest::Method::POST,
                &format!("/containers/{id}/wait?condition=not-running"),
                None,
            ),
        )
        .await
        .map_err(|_| "GPU_PROBE_FAILED")??
        .ok_or("GPU_PROBE_FAILED")?;
        if wait["StatusCode"] != 0 {
            return Err("GPU_PROBE_FAILED");
        }
        let bytes = docker.logs(&id).await?;
        let out = stdout(&bytes)?;
        let text = std::str::from_utf8(&out).map_err(|_| "GPU_PROBE_FAILED")?;
        let uuids = text
            .lines()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>();
        if uuids.len() != 1 || !uuids[0].eq_ignore_ascii_case(gpu) {
            return Err("GPU_PASSTHROUGH_MISMATCH");
        }
        Ok::<(), Error>(())
    }
    .await;
    let inspected = docker
        .request(
            reqwest::Method::GET,
            &format!("/containers/{id}/json"),
            None,
        )
        .await?
        .ok_or("GPU_PROBE_FAILED")?;
    if !owned(&inspected, node, gpu, &name, image) {
        return Err("CONTAINER_OWNERSHIP_MISMATCH");
    }
    if inspected["State"]["Running"] == true {
        docker
            .request(
                reqwest::Method::POST,
                &format!("/containers/{id}/stop?t=2"),
                None,
            )
            .await?
            .ok_or("GPU_PROBE_FAILED")?;
    }
    let stopped = docker
        .request(
            reqwest::Method::GET,
            &format!("/containers/{id}/json"),
            None,
        )
        .await?
        .ok_or("GPU_PROBE_FAILED")?;
    if !owned(&stopped, node, gpu, &name, image) || stopped["State"]["Running"] != false {
        return Err("GPU_PROBE_STOP_UNCONFIRMED");
    }
    docker
        .request(
            reqwest::Method::DELETE,
            &format!("/containers/{id}?v=false"),
            None,
        )
        .await?;
    if docker
        .request(
            reqwest::Method::GET,
            &format!("/containers/{id}/json"),
            None,
        )
        .await?
        .is_some()
    {
        return Err("GPU_PROBE_STOP_UNCONFIRMED");
    }
    outcome
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    struct Fake {
        current: Mutex<Option<Value>>,
        calls: Mutex<Vec<String>>,
        output: String,
    }
    impl Docker for Fake {
        async fn notebook_ready(&self, _: &str, _: &str, _: &str) -> Result<bool> {
            Ok(false)
        }
        async fn gpu_processes(&self) -> Result<Vec<super::super::notebook_executor::GpuProcess>> {
            Ok(vec![])
        }
        async fn logs(&self, _: &str) -> Result<Vec<u8>> {
            let b = self.output.as_bytes();
            let mut bytes = vec![1, 0, 0, 0];
            bytes.extend((b.len() as u32).to_be_bytes());
            bytes.extend(b);
            Ok(bytes)
        }
        async fn request(
            &self,
            m: reqwest::Method,
            path: &str,
            body: Option<Value>,
        ) -> Result<Option<Value>> {
            self.calls.lock().unwrap().push(format!("{m} {path}"));
            if path.contains("/create?") {
                let b = body.unwrap();
                assert_eq!(b["HostConfig"]["NetworkMode"], "none");
                assert_eq!(b["HostConfig"]["ReadonlyRootfs"], true);
                assert_eq!(b["HostConfig"]["Privileged"], false);
                assert!(b["HostConfig"].get("Mounts").is_none());
                assert_eq!(
                    b["HostConfig"]["DeviceRequests"][0]["DeviceIDs"],
                    json!([GPU])
                );
                assert_eq!(b["Entrypoint"], json!(["/usr/bin/timeout"]));
                assert_eq!(b["Cmd"][3], "/usr/bin/nvidia-smi");
                *self.current.lock().unwrap() = Some(
                    json!({"Id":"c".repeat(64),"Config":{"Image":b["Image"],"Labels":b["Labels"]},"HostConfig":b["HostConfig"],"State":{"Running":false,"Status":"created"}}),
                );
                return Ok(Some(json!({"Id":"c".repeat(64)})));
            }
            if path.ends_with("/json") {
                return Ok(self.current.lock().unwrap().clone());
            }
            if path.ends_with("/start") {
                self.current.lock().unwrap().as_mut().unwrap()["State"]["Running"] = json!(true);
                return Ok(Some(json!({})));
            }
            if path.contains("/wait?") {
                let mut v = self.current.lock().unwrap();
                v.as_mut().unwrap()["State"]["Running"] = json!(false);
                v.as_mut().unwrap()["State"]["Status"] = json!("exited");
                return Ok(Some(json!({"StatusCode":0})));
            }
            if m == reqwest::Method::DELETE {
                assert!(path.contains(&"c".repeat(64)));
                *self.current.lock().unwrap() = None;
                return Ok(Some(json!({})));
            }
            panic!("Unexpected synthetic probe operation")
        }
    }
    const NODE: &str = "10000000-0000-4000-8000-000000000002";
    const GPU: &str = "GPU-11111111-2222-3333-4444-555555555555";
    const IMAGE: &str =
        "fixture/image@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    #[tokio::test]
    async fn exact_device_passthrough_proof_is_required_and_probe_is_removed() {
        let f = Fake {
            current: Mutex::new(None),
            calls: Mutex::new(vec![]),
            output: format!("{GPU}\n"),
        };
        run(&f, NODE, IMAGE, GPU).await.unwrap();
        assert!(f.current.lock().unwrap().is_none());
        assert_eq!(
            f.calls
                .lock()
                .unwrap()
                .iter()
                .filter(|c| c.contains("/create?"))
                .count(),
            1
        );
    }
    #[tokio::test]
    async fn false_gpu_proof_never_advertises_capability() {
        let f = Fake {
            current: Mutex::new(None),
            calls: Mutex::new(vec![]),
            output: "GPU-aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee\n".into(),
        };
        assert_eq!(
            run(&f, NODE, IMAGE, GPU).await.unwrap_err(),
            "GPU_PASSTHROUGH_MISMATCH"
        );
        assert!(f.current.lock().unwrap().is_none());
    }
    #[tokio::test]
    async fn foreign_container_is_never_stopped_or_deleted() {
        let f = Fake {
            current: Mutex::new(Some(
                json!({"Id":"f".repeat(64),"Config":{"Image":IMAGE,"Labels":{"io.cbgpu.managed":"legacy-gpu"}},"HostConfig":{"NetworkMode":"none","Privileged":false,"ReadonlyRootfs":true,"DeviceRequests":[{"DeviceIDs":[GPU]}]},"State":{"Running":true}}),
            )),
            calls: Mutex::new(vec![]),
            output: GPU.into(),
        };
        assert_eq!(
            run(&f, NODE, IMAGE, GPU).await.unwrap_err(),
            "CONTAINER_OWNERSHIP_MISMATCH"
        );
        assert!(
            f.calls
                .lock()
                .unwrap()
                .iter()
                .all(|c| c.starts_with("GET "))
        );
    }
}
