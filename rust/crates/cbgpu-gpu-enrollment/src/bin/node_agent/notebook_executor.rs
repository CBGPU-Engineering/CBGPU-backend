//! Opt-in notebook sandbox. Docker API is pinned to a local Unix socket; no shell or user-supplied host paths.
use super::{
    notebook_docker_stream::{self, Pipe},
    notebook_http,
};
use cbgpu_gpu_enrollment::{
    hex,
    notebooks::protocol::{NotebookResult, NotebookSpec},
    valid_hardware_uuid,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::Digest;
use std::{
    collections::BTreeMap,
    future::Future,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
pub type Error = &'static str;
pub type Result<T> = std::result::Result<T, Error>;
#[derive(Clone)]
pub struct GpuProcess {
    pub uuid: String,
    pub pid: u32,
}
pub trait Docker: Send + Sync {
    fn logs(&self, _container: &str) -> impl Future<Output = Result<Vec<u8>>> + Send {
        async { Err("GPU_PROBE_FAILED") }
    }

    fn open_stream(&self, _container: &str) -> impl Future<Output = Result<Pipe>> + Send {
        async { Err("DOCKER_HIJACK_UNAVAILABLE") }
    }
    fn notebook_ready(
        &self,
        container: &str,
        token: &str,
        job: &str,
    ) -> impl Future<Output = Result<bool>> + Send;
    fn gpu_processes(&self) -> impl Future<Output = Result<Vec<GpuProcess>>> + Send;

    fn request(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
    ) -> impl Future<Output = Result<Option<Value>>> + Send;
}
pub struct LocalDocker {
    http: reqwest::Client,
}
impl LocalDocker {
    pub fn open() -> Result<Self> {
        #[cfg(unix)]
        {
            Ok(Self {
                http: reqwest::Client::builder()
                    .no_proxy()
                    .unix_socket("/var/run/docker.sock")
                    .redirect(reqwest::redirect::Policy::none())
                    .timeout(std::time::Duration::from_secs(30))
                    .build()
                    .map_err(|_| "DOCKER_UNAVAILABLE")?,
            })
        }
        #[cfg(not(unix))]
        {
            Err("UNSUPPORTED_PLATFORM")
        }
    }
}
impl Docker for LocalDocker {
    async fn logs(&self, container: &str) -> Result<Vec<u8>> {
        if container.len() != 64 || !container.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err("CONTAINER_OWNERSHIP_MISMATCH");
        }
        let mut response = self
            .http
            .get(format!(
                "http://localhost/v1.43/containers/{container}/logs?stdout=true&stderr=false"
            ))
            .send()
            .await
            .map_err(|_| "DOCKER_UNAVAILABLE")?;
        if !response.status().is_success() {
            return Err("DOCKER_FAILED");
        }
        let mut bytes = vec![];
        while let Some(chunk) = response.chunk().await.map_err(|_| "DOCKER_FAILED")? {
            if bytes.len() + chunk.len() > 32768 {
                return Err("GPU_PROBE_FAILED");
            }
            bytes.extend(chunk)
        }
        Ok(bytes)
    }
    async fn open_stream(&self, container: &str) -> Result<Pipe> {
        notebook_docker_stream::open(&self.http, container).await
    }
    async fn notebook_ready(&self, container: &str, token: &str, job: &str) -> Result<bool> {
        if !uuid(job) || token.len() != 64 {
            return Err("INVALID_NOTEBOOK_TARGET");
        }
        let pipe = match self.open_stream(container).await {
            Ok(p) => p,
            Err(_) => return Ok(false),
        };
        let req = hyper::Request::builder()
            .method("GET")
            .uri(format!("/notebooks/{job}/api"))
            .header("Host", "localhost")
            .header("Authorization", format!("token {token}"))
            .body(http_body_util::Full::new(bytes::Bytes::new()))
            .map_err(|_| "INVALID_NOTEBOOK_TARGET")?;
        let response = match tokio::time::timeout(
            std::time::Duration::from_secs(3),
            notebook_http::request(pipe, req),
        )
        .await
        {
            Ok(Ok(r)) => r,
            _ => return Ok(false),
        };
        if response.status() != 200 {
            return Ok(false);
        }
        let body = notebook_http::body(response.into_body(), 65536).await?;
        let value: Value = serde_json::from_slice(&body).map_err(|_| "UPSTREAM_UNAVAILABLE")?;
        Ok(value["version"]
            .as_str()
            .is_some_and(|v| !v.is_empty() && v.len() < 64))
    }
    async fn gpu_processes(&self) -> Result<Vec<GpuProcess>> {
        use std::process::Stdio;
        let child = tokio::process::Command::new(cbgpu_gpu_enrollment::nvidia_path::trusted()?)
            .args([
                "--query-compute-apps=gpu_uuid,pid",
                "--format=csv,noheader,nounits",
            ])
            .env_clear()
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(|_| "NVIDIA_UNAVAILABLE")?;
        let out = tokio::time::timeout(std::time::Duration::from_secs(5), child.wait_with_output())
            .await
            .map_err(|_| "NVIDIA_UNAVAILABLE")?
            .map_err(|_| "NVIDIA_UNAVAILABLE")?;
        if !out.status.success() || out.stdout.len() > 16384 {
            return Err("NVIDIA_UNAVAILABLE");
        }
        std::str::from_utf8(&out.stdout)
            .map_err(|_| "NVIDIA_UNAVAILABLE")?
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|line| {
                let mut fields = line.split(',').map(str::trim);
                let uuid = fields
                    .next()
                    .filter(|v| valid_hardware_uuid(v))
                    .ok_or("NVIDIA_UNAVAILABLE")?;
                let pid = fields
                    .next()
                    .and_then(|v| v.parse::<u32>().ok())
                    .filter(|p| *p > 0)
                    .ok_or("NVIDIA_UNAVAILABLE")?;
                if fields.next().is_some() {
                    return Err("NVIDIA_UNAVAILABLE");
                }
                Ok(GpuProcess {
                    uuid: uuid.into(),
                    pid,
                })
            })
            .collect()
    }
    async fn request(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<Option<Value>> {
        let mut req = self
            .http
            .request(method, format!("http://localhost/v1.43{path}"));
        if let Some(body) = body {
            req = req.json(&body)
        }
        let r = req.send().await.map_err(|_| "DOCKER_UNAVAILABLE")?;
        if r.status() == 404 {
            return Ok(None);
        }
        if !r.status().is_success() {
            return Err("DOCKER_FAILED");
        }
        if r.status() == 204 {
            return Ok(Some(json!({})));
        }
        let bytes = r.bytes().await.map_err(|_| "DOCKER_UNAVAILABLE")?;
        if bytes.len() > 2_000_000 {
            return Err("DOCKER_FAILED");
        }
        Ok(Some(
            serde_json::from_slice(&bytes).map_err(|_| "DOCKER_FAILED")?,
        ))
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    spec: NotebookSpec,
    token: String,
    container_id: Option<String>,
    proxy_sha: String,
    state: String,
}
pub struct Executor<D: Docker> {
    docker: Arc<D>,
    root: PathBuf,
    node: String,
    image: String,
    deadlines: Mutex<BTreeMap<String, tokio::time::Instant>>,
    operations: Mutex<BTreeMap<String, Arc<tokio::sync::Mutex<()>>>>,
    hardware_locks: Mutex<BTreeMap<String, HardwareLock>>,
    hardware_lock_root: PathBuf,
    proxy_sha: String,
    proven_gpus: Mutex<std::collections::BTreeSet<String>>,
}
fn uuid(v: &str) -> bool {
    v.len() == 36
        && v.bytes().enumerate().all(|(i, b)| {
            if [8, 13, 18, 23].contains(&i) {
                b == b'-'
            } else {
                b.is_ascii_hexdigit() && !b.is_ascii_uppercase()
            }
        })
}
fn image_valid(v: &str) -> bool {
    v.rsplit_once("@sha256:").is_some_and(|(name, hash)| {
        !name.is_empty()
            && name.len() < 200
            && name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"/._:-".contains(&b))
            && hash.len() == 64
            && hash
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    })
}
fn name(job: &str) -> String {
    format!("cbgpu-node-notebook-{job}")
}
fn labels(s: &NotebookSpec) -> Value {
    json!({"io.cbgpu.managed":"notebook-v3","io.cbgpu.node":s.node_id,"io.cbgpu.school":s.school_id,"io.cbgpu.owner":s.owner_id,"io.cbgpu.resource":s.resource_id,"io.cbgpu.reservation":s.reservation_id,"io.cbgpu.generation":s.job_id,"io.cbgpu.relay":s.relay_session_id})
}
struct HardwareLock {
    file: std::fs::File,
    path: PathBuf,
}
impl HardwareLock {
    fn remove_after_absence(&self) {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if let (Ok(open), Ok(current)) =
                (self.file.metadata(), std::fs::symlink_metadata(&self.path))
                && open.dev() == current.dev()
                && open.ino() == current.ino()
            {
                let _ = std::fs::remove_file(&self.path);
            }
        }
    }
}
impl<D: Docker> Executor<D> {
    #[cfg(test)]
    #[allow(dead_code)]
    pub fn with_test_locks(mut self) -> Self {
        self.hardware_lock_root = self.root.join("test-hardware-locks");
        std::fs::create_dir_all(&self.hardware_lock_root).unwrap();
        self
    }
    #[cfg(test)]
    #[allow(dead_code)]
    pub fn with_shared_test_locks(mut self, path: &Path) -> Self {
        self.hardware_lock_root = path.into();
        std::fs::create_dir_all(path).unwrap();
        self
    }
    fn lock_hardware(&self, s: &NotebookSpec) -> Result<()> {
        let mut locks = self
            .hardware_locks
            .lock()
            .map_err(|_| "PRIVATE_FILE_INVALID")?;
        if locks.contains_key(&s.job_id) {
            return Ok(());
        }
        let path = self.hardware_lock_root.join(format!(
            ".cbgpu-notebook-{}.lock",
            s.hardware_uuid.to_ascii_lowercase()
        ));
        let mut o = std::fs::OpenOptions::new();
        o.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            o.mode(0o600)
                .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
        }
        let file = o.open(&path).map_err(|_| "GPU_BUSY")?;
        let m = file.metadata().map_err(|_| "GPU_BUSY")?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::{MetadataExt, PermissionsExt};
            if !m.is_file()
                || m.uid() != unsafe { libc::geteuid() }
                || m.permissions().mode() & 0o077 != 0
                || m.nlink() != 1
            {
                return Err("GPU_BUSY");
            }
        }
        fs2::FileExt::try_lock_exclusive(&file).map_err(|_| "GPU_BUSY")?;
        locks.insert(s.job_id.clone(), HardwareLock { file, path });
        Ok(())
    }
    fn unlock_hardware(&self, job: &str) -> Result<()> {
        if let Some(lock) = self
            .hardware_locks
            .lock()
            .map_err(|_| "PRIVATE_FILE_INVALID")?
            .remove(job)
        {
            lock.remove_after_absence();
        }
        Ok(())
    }

    pub fn open(docker: Arc<D>, root: &Path, node: &str, image: &str) -> Result<Self> {
        if !root.is_absolute() || !uuid(node) || !image_valid(image) {
            return Err("INVALID_NOTEBOOK_POLICY");
        }
        for p in root.ancestors() {
            if std::fs::symlink_metadata(p).is_ok_and(|m| m.file_type().is_symlink()) {
                return Err("PRIVATE_FILE_INVALID");
            }
        }
        let mut b = std::fs::DirBuilder::new();
        b.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            b.mode(0o700);
        }
        b.create(root).map_err(|_| "PRIVATE_FILE_UNAVAILABLE")?;
        let m = std::fs::symlink_metadata(root).map_err(|_| "PRIVATE_FILE_UNAVAILABLE")?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::{MetadataExt, PermissionsExt};
            if !m.is_dir()
                || m.permissions().mode() & 0o077 != 0
                || m.uid() != unsafe { libc::geteuid() }
            {
                return Err("PRIVATE_FILE_INVALID");
            }
        }
        Ok(Self {
            docker,
            root: root.into(),
            node: node.into(),
            image: image.into(),
            deadlines: Mutex::new(BTreeMap::new()),
            operations: Mutex::new(BTreeMap::new()),
            hardware_locks: Mutex::new(BTreeMap::new()),
            hardware_lock_root: PathBuf::from("/dev/shm"),
            proxy_sha: stage_proxy(root)?,
            proven_gpus: Mutex::new(std::collections::BTreeSet::new()),
        })
    }
    fn operation(&self, job: &str) -> Result<Arc<tokio::sync::Mutex<()>>> {
        if !uuid(job) {
            return Err("INVALID_NOTEBOOK_SPEC");
        }
        Ok(self
            .operations
            .lock()
            .map_err(|_| "PRIVATE_FILE_INVALID")?
            .entry(job.into())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone())
    }
    fn path(&self, job: &str) -> Result<PathBuf> {
        if !uuid(job) {
            return Err("INVALID_NOTEBOOK_SPEC");
        }
        Ok(self.root.join(format!("{job}.json")))
    }
    fn read(&self, job: &str) -> Result<Option<Entry>> {
        let p = self.path(job)?;
        let mut o = std::fs::OpenOptions::new();
        o.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            o.custom_flags(libc::O_NOFOLLOW);
        }
        let f = match o.open(p) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err("PRIVATE_FILE_INVALID"),
        };
        let m = f.metadata().map_err(|_| "PRIVATE_FILE_INVALID")?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if m.permissions().mode() & 0o077 != 0 {
                return Err("PRIVATE_FILE_INVALID");
            }
        }
        if !m.is_file() || m.len() > 32768 {
            return Err("PRIVATE_FILE_INVALID");
        }
        let entry: Entry = serde_json::from_reader(f).map_err(|_| "PRIVATE_FILE_INVALID")?;
        if entry.spec.node_id != self.node || entry.spec.job_id != job {
            return Err("JOURNAL_BINDING_MISMATCH");
        }
        Ok(Some(entry))
    }
    fn save(&self, e: &Entry) -> Result<()> {
        use std::io::Write;
        let p = self.path(&e.spec.job_id)?;
        let tmp = self.root.join(format!(".{}.tmp", hex(&random()?)));
        let mut o = std::fs::OpenOptions::new();
        o.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            o.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        }
        let mut f = o.open(&tmp).map_err(|_| "PRIVATE_FILE_UNAVAILABLE")?;
        f.write_all(&serde_json::to_vec(e).map_err(|_| "PRIVATE_FILE_INVALID")?)
            .map_err(|_| "PRIVATE_FILE_UNAVAILABLE")?;
        f.sync_all().map_err(|_| "PRIVATE_FILE_UNAVAILABLE")?;
        std::fs::rename(tmp, p).map_err(|_| "PRIVATE_FILE_UNAVAILABLE")?;
        std::fs::File::open(&self.root)
            .and_then(|f| f.sync_all())
            .map_err(|_| "PRIVATE_FILE_UNAVAILABLE")
    }
    fn valid(&self, s: &NotebookSpec, now: i64) -> bool {
        s.node_id == self.node
            && s.image_digest == self.image
            && [
                &s.job_id,
                &s.node_id,
                &s.resource_id,
                &s.owner_id,
                &s.reservation_id,
                &s.relay_session_id,
            ]
            .into_iter()
            .all(|v| uuid(v))
            && !s.school_id.is_empty()
            && s.school_id.len() <= 100
            && valid_hardware_uuid(&s.hardware_uuid)
            && s.not_before <= now
            && s.deadline > now
            && s.deadline - s.not_before <= 14400
            && (100..=8000).contains(&s.limits.cpu_millis)
            && (536870912..=34359738368).contains(&s.limits.memory_bytes)
            && (32..=1024).contains(&s.limits.pids)
            && (67108864..=17179869184).contains(&s.limits.tmpfs_bytes)
    }
    pub async fn preflight(&self) -> Result<()> {
        let info = self
            .docker
            .request(reqwest::Method::GET, "/info", None)
            .await?
            .ok_or("DOCKER_UNAVAILABLE")?;
        if info["OSType"] != "linux" || info["Runtimes"]["nvidia"].is_null() {
            return Err("NVIDIA_UNAVAILABLE");
        }
        let image = self
            .docker
            .request(
                reqwest::Method::GET,
                &format!("/images/{}/json", self.image),
                None,
            )
            .await?
            .ok_or("IMAGE_NOT_AVAILABLE")?;
        if image["Config"]["Volumes"]
            .as_object()
            .is_some_and(|volumes| {
                volumes
                    .keys()
                    .any(|p| !["/workspace", "/tmp"].contains(&p.as_str()))
            })
        {
            return Err("UNBOUNDED_IMAGE_STORAGE");
        }
        if image["Os"] != "linux"
            || !image["RepoDigests"]
                .as_array()
                .is_some_and(|d| d.iter().any(|v| v == &self.image))
        {
            return Err("IMAGE_NOT_AVAILABLE");
        }
        Ok(())
    }
    pub async fn prove_hardware(&self, hardware: &[String]) -> Result<()> {
        self.preflight().await?;
        for gpu in hardware {
            if !valid_hardware_uuid(gpu) {
                return Err("NVIDIA_UNAVAILABLE");
            }
            if self
                .proven_gpus
                .lock()
                .map_err(|_| "PRIVATE_FILE_INVALID")?
                .contains(gpu)
            {
                continue;
            }
            let mut spec = NotebookSpec {
                job_id: self.node.clone(),
                node_id: self.node.clone(),
                owner_id: self.node.clone(),
                resource_id: self.node.clone(),
                reservation_id: self.node.clone(),
                relay_session_id: self.node.clone(),
                school_id: "preflight".into(),
                hardware_uuid: gpu.clone(),
                image_digest: self.image.clone(),
                not_before: 0,
                deadline: 0,
                limits: cbgpu_gpu_enrollment::notebooks::protocol::Limits {
                    cpu_millis: 500,
                    memory_bytes: 268435456,
                    pids: 32,
                    tmpfs_bytes: 16777216,
                },
            };
            spec.job_id = format!(
                "00000000-0000-4000-8000-{}",
                &format!("{:x}", sha2::Sha256::digest(gpu.as_bytes()))[..12]
            );
            self.lock_hardware(&spec)?;
            self.assert_gpu_free(&spec).await?;
            let proof =
                super::notebook_gpu_probe::run(self.docker.as_ref(), &self.node, &self.image, gpu)
                    .await;
            if proof.is_ok() {
                self.unlock_hardware(&spec.job_id)?;
                self.proven_gpus
                    .lock()
                    .map_err(|_| "PRIVATE_FILE_INVALID")?
                    .insert(gpu.clone());
            }
            proof?;
        }
        Ok(())
    }
    fn matches(&self, e: &Entry, v: &Value) -> bool {
        e.spec.node_id == self.node
            && v["Config"]["Labels"].as_object().is_some_and(|actual| {
                labels(&e.spec).as_object().is_some_and(|expected| {
                    expected.iter().all(|(k, val)| actual.get(k) == Some(val))
                        && actual.keys().filter(|k| k.starts_with("io.cbgpu.")).count()
                            == expected.len()
                })
            })
            && self.proxy_mount_matches(e, v)
            && v["Config"]["Image"] == self.image
            && v["HostConfig"]["ReadonlyRootfs"] == true
            && v["HostConfig"]["Privileged"] == false
            && v["HostConfig"]["NetworkMode"] == "none"
            && v["HostConfig"]["PortBindings"]
                .as_object()
                .is_none_or(|p| p.is_empty())
            && v["HostConfig"]["PublishAllPorts"] != true
            && v["HostConfig"]["DeviceRequests"]
                .as_array()
                .is_some_and(|a| a.len() == 1 && a[0]["DeviceIDs"] == json!([e.spec.hardware_uuid]))
            && v["Id"].as_str().is_some_and(|id| {
                id.len() == 64
                    && id.bytes().all(|b| b.is_ascii_hexdigit())
                    && e.container_id.as_ref().is_none_or(|old| old == id)
            })
    }
    async fn inspect(&self, e: &Entry) -> Result<Option<Value>> {
        let v = self
            .docker
            .request(
                reqwest::Method::GET,
                &format!("/containers/{}/json", name(&e.spec.job_id)),
                None,
            )
            .await?;
        if v.as_ref().is_some_and(|v| !self.matches(e, v)) {
            return Err("CONTAINER_OWNERSHIP_MISMATCH");
        }
        Ok(v)
    }
    fn proxy_path(&self, sha: &str) -> Result<PathBuf> {
        if sha.len() != 64
            || !sha
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        {
            return Err("INVALID_PROXY_BINARY");
        }
        Ok(self.root.join("proxy-bin").join(format!("{sha}.elf")))
    }
    fn proxy_mount_matches(&self, e: &Entry, v: &Value) -> bool {
        let Ok(path) = self.proxy_path(&e.proxy_sha) else {
            return false;
        };
        v["Mounts"].as_array().is_some_and(|mounts| {
            mounts.iter().any(|m| {
                m["Type"] == "bind"
                    && m["Source"] == path.to_string_lossy().as_ref()
                    && m["Destination"] == "/usr/local/bin/cbgpu-node-proxy"
                    && m["RW"] == false
            }) && mounts.iter().all(|m| {
                (m["Type"] == "tmpfs"
                    && ["/tmp", "/workspace"]
                        .iter()
                        .any(|p| m["Destination"] == *p))
                    || (m["Type"] == "bind"
                        && m["Destination"] == "/usr/local/bin/cbgpu-node-proxy"
                        && m["Source"] == path.to_string_lossy().as_ref()
                        && m["RW"] == false)
            })
        })
    }
    async fn assert_gpu_free(&self, s: &NotebookSpec) -> Result<()> {
        let rows = self
            .docker
            .request(reqwest::Method::GET, "/containers/json?all=false", None)
            .await?
            .ok_or("DOCKER_FAILED")?;
        let rows = rows
            .as_array()
            .filter(|r| r.len() <= 128)
            .ok_or("DOCKER_FAILED")?;
        let mut owned_pids = std::collections::BTreeSet::new();
        for row in rows {
            let id = row["Id"]
                .as_str()
                .filter(|id| id.len() == 64 && id.bytes().all(|b| b.is_ascii_hexdigit()))
                .ok_or("DOCKER_FAILED")?;
            let Some(v) = self
                .docker
                .request(
                    reqwest::Method::GET,
                    &format!("/containers/{id}/json"),
                    None,
                )
                .await?
            else {
                continue;
            };
            if v["State"]["Running"] != true {
                continue;
            }
            if self
                .read(&s.job_id)?
                .as_ref()
                .is_some_and(|e| self.matches(e, &v))
            {
                let top = self
                    .docker
                    .request(
                        reqwest::Method::GET,
                        &format!("/containers/{id}/top?ps_args=-eo%20pid"),
                        None,
                    )
                    .await?
                    .ok_or("DOCKER_FAILED")?;
                let titles = top["Titles"].as_array().ok_or("DOCKER_FAILED")?;
                let index = titles
                    .iter()
                    .position(|v| v == "PID")
                    .ok_or("DOCKER_FAILED")?;
                for p in top["Processes"]
                    .as_array()
                    .filter(|p| p.len() <= 4096)
                    .ok_or("DOCKER_FAILED")?
                {
                    owned_pids.insert(
                        p[index]
                            .as_str()
                            .and_then(|p| p.parse::<u32>().ok())
                            .ok_or("DOCKER_FAILED")?,
                    );
                }
                continue;
            }
            let requested = v["HostConfig"]["DeviceRequests"]
                .as_array()
                .is_some_and(|requests| {
                    requests.iter().any(|r| {
                        r["Driver"] == "nvidia"
                            && (r["Count"] == -1
                                || r["DeviceIDs"].as_array().is_none_or(|ids| {
                                    ids.iter().any(|id| {
                                        id == &s.hardware_uuid
                                            || id.as_str().is_none_or(|id| !id.starts_with("GPU-"))
                                    })
                                }))
                    })
                });
            let ambiguous = v["HostConfig"]["Runtime"] == "nvidia"
                && v["Config"]["Env"].as_array().is_some_and(|env| {
                    env.iter().any(|v| {
                        v.as_str().is_some_and(|v| {
                            v.starts_with("NVIDIA_VISIBLE_DEVICES=")
                                && !v.ends_with("=none")
                                && !v.ends_with("=void")
                        })
                    })
                });
            if requested || ambiguous {
                return Err("GPU_BUSY");
            }
        }
        if self
            .docker
            .gpu_processes()
            .await?
            .iter()
            .any(|p| p.uuid.eq_ignore_ascii_case(&s.hardware_uuid) && !owned_pids.contains(&p.pid))
        {
            return Err("GPU_BUSY");
        }
        Ok(())
    }
    async fn check_host_capacity(&self, s: &NotebookSpec) -> Result<()> {
        let info = self
            .docker
            .request(reqwest::Method::GET, "/info", None)
            .await?
            .ok_or("DOCKER_UNAVAILABLE")?;
        let total = info["MemTotal"]
            .as_u64()
            .ok_or("HOST_CAPACITY_UNAVAILABLE")?;
        let cpu = info["NCPU"]
            .as_u64()
            .and_then(|v| v.checked_mul(1000))
            .ok_or("HOST_CAPACITY_UNAVAILABLE")?;
        let (mut used_ram, mut used_cpu) = (s.limits.memory_bytes, u64::from(s.limits.cpu_millis));
        for job in self.journal_jobs()? {
            if job == s.job_id {
                continue;
            }
            if let Some(e) = self.read(&job)?
                && e.state != "STOPPED"
            {
                used_ram = used_ram
                    .checked_add(e.spec.limits.memory_bytes)
                    .ok_or("HOST_CAPACITY_UNAVAILABLE")?;
                used_cpu = used_cpu
                    .checked_add(u64::from(e.spec.limits.cpu_millis))
                    .ok_or("HOST_CAPACITY_UNAVAILABLE")?;
            }
        }
        if used_ram > total.saturating_mul(4) / 5 || used_cpu > cpu {
            return Err("HOST_CAPACITY_UNAVAILABLE");
        }
        Ok(())
    }
    pub async fn start(&self, s: &NotebookSpec, now: i64) -> Result<NotebookResult> {
        let _operation = self.operation(&s.job_id)?.lock_owned().await;
        if !self.valid(s, now) {
            return Err("INVALID_NOTEBOOK_SPEC");
        }
        self.deadlines
            .lock()
            .map_err(|_| "PRIVATE_FILE_INVALID")?
            .entry(s.job_id.clone())
            .or_insert(
                tokio::time::Instant::now()
                    + std::time::Duration::from_secs(
                        (s.deadline - now)
                            .try_into()
                            .map_err(|_| "INVALID_NOTEBOOK_SPEC")?,
                    ),
            );
        self.lock_hardware(s)?;
        self.preflight().await?;
        self.check_host_capacity(s).await?;
        self.assert_gpu_free(s).await?;
        let mut e = if let Some(old) = self.read(&s.job_id)? {
            if serde_json::to_value(&old.spec).ok() != serde_json::to_value(s).ok() {
                return Err("NOTEBOOK_IDENTITY_CHANGED");
            }
            if old.state == "STOPPED" {
                return Err("NOTEBOOK_ALREADY_STOPPED");
            }
            old
        } else {
            let e = Entry {
                spec: s.clone(),
                token: hex(&random()?),
                container_id: None,
                proxy_sha: self.proxy_sha.clone(),
                state: "PREPARED".into(),
            };
            self.save(&e)?;
            e
        };
        let mut existing = self.inspect(&e).await?;
        if existing.is_none() {
            let body = sandbox(s, &e.token, now, &self.proxy_path(&e.proxy_sha)?);
            let made = self
                .docker
                .request(
                    reqwest::Method::POST,
                    &format!("/containers/create?name={}", name(&s.job_id)),
                    Some(body),
                )
                .await?
                .ok_or("DOCKER_FAILED")?;
            let id = made["Id"]
                .as_str()
                .filter(|id| id.len() == 64 && id.bytes().all(|b| b.is_ascii_hexdigit()))
                .ok_or("DOCKER_FAILED")?;
            e.container_id = Some(id.into());
            self.save(&e)?;
            existing = self.inspect(&e).await?;
        }
        let current = existing.ok_or("DOCKER_FAILED")?;
        if e.container_id.is_none() {
            e.container_id = current["Id"].as_str().map(str::to_owned);
            self.save(&e)?
        }
        if current["State"]["Running"] != true && e.state == "RUNNING" {
            return Err("NOTEBOOK_ALREADY_STOPPED");
        }
        if !self
            .deadlines
            .lock()
            .map_err(|_| "PRIVATE_FILE_INVALID")?
            .get(&s.job_id)
            .is_some_and(|d| *d > tokio::time::Instant::now())
        {
            return Err("CONTROL_LEASE_EXPIRED");
        }
        if current["State"]["Running"] != true {
            self.docker
                .request(
                    reqwest::Method::POST,
                    &format!(
                        "/containers/{}/start",
                        e.container_id.as_deref().ok_or("DOCKER_FAILED")?
                    ),
                    None,
                )
                .await?
                .ok_or("DOCKER_FAILED")?;
        }
        let v = self.inspect(&e).await?.ok_or("DOCKER_FAILED")?;
        if v["State"]["Running"] != true {
            return Err("NOTEBOOK_NOT_RUNNING");
        }
        let container = v["Id"].as_str().ok_or("CONTAINER_OWNERSHIP_MISMATCH")?;
        tokio::time::timeout(std::time::Duration::from_secs(15), async {
            loop {
                if !self
                    .deadlines
                    .lock()
                    .map_err(|_| "PRIVATE_FILE_INVALID")?
                    .get(&s.job_id)
                    .is_some_and(|d| *d > tokio::time::Instant::now())
                {
                    return Err("CONTROL_LEASE_EXPIRED");
                }
                if self
                    .docker
                    .notebook_ready(container, &e.token, &s.job_id)
                    .await?
                {
                    return Ok(());
                }
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            }
        })
        .await
        .map_err(|_| "NOTEBOOK_NOT_READY")??;
        e.state = "RUNNING".into();
        self.save(&e)?;
        Ok(result(s, "RUNNING"))
    }
    pub async fn stop(&self, job: &str) -> Result<NotebookResult> {
        let _operation = self.operation(job)?.lock_owned().await;
        let mut e = self.read(job)?.ok_or("UNKNOWN_NOTEBOOK")?;
        if let Some(v) = self.inspect(&e).await? {
            if v["State"]["Running"] == true {
                self.docker
                    .request(
                        reqwest::Method::POST,
                        &format!(
                            "/containers/{}/stop?t=10",
                            v["Id"].as_str().ok_or("DOCKER_FAILED")?
                        ),
                        None,
                    )
                    .await?
                    .ok_or("DOCKER_FAILED")?;
            }
            self.docker
                .request(
                    reqwest::Method::DELETE,
                    &format!(
                        "/containers/{}?v=false",
                        v["Id"].as_str().ok_or("DOCKER_FAILED")?
                    ),
                    None,
                )
                .await?;
            if self.inspect(&e).await?.is_some() {
                return Err("NOTEBOOK_STOP_UNCONFIRMED");
            }
        }
        e.state = "STOPPED".into();
        self.save(&e)?;
        self.unlock_hardware(job)?;
        Ok(result(&e.spec, "STOPPED"))
    }
    pub fn invalidate(&self, job: &str) -> Result<()> {
        if !uuid(job) {
            return Err("INVALID_NOTEBOOK_SPEC");
        }
        self.deadlines
            .lock()
            .map_err(|_| "PRIVATE_FILE_INVALID")?
            .insert(job.into(), tokio::time::Instant::now());
        Ok(())
    }
    /// Caller must verify a fresh protocol-v3 central signature before extending this fence.
    pub fn authorize_until(&self, job: &str, expires: i64, now: i64) -> Result<()> {
        if !uuid(job) || expires <= now || expires > now.saturating_add(120) {
            return Err("INVALID_CONTROL_AUTHORITY");
        }
        self.deadlines
            .lock()
            .map_err(|_| "PRIVATE_FILE_INVALID")?
            .insert(
                job.into(),
                tokio::time::Instant::now()
                    + std::time::Duration::from_secs((expires - now) as u64),
            );
        Ok(())
    }
    pub async fn stop_bound(&self, s: &NotebookSpec) -> Result<NotebookResult> {
        if s.node_id != self.node {
            return Err("NOTEBOOK_IDENTITY_CHANGED");
        }
        let Some(e) = self.read(&s.job_id)? else {
            if s.node_id != self.node || !uuid(&s.job_id) {
                return Err("INVALID_NOTEBOOK_SPEC");
            }
            if self
                .docker
                .request(
                    reqwest::Method::GET,
                    &format!("/containers/{}/json", name(&s.job_id)),
                    None,
                )
                .await?
                .is_none()
            {
                self.unlock_hardware(&s.job_id)?;
                return Ok(result(s, "STOPPED"));
            }
            return Err("CONTAINER_OWNERSHIP_MISMATCH");
        };
        if serde_json::to_value(&e.spec).ok() != serde_json::to_value(s).ok() {
            return Err("NOTEBOOK_IDENTITY_CHANGED");
        }
        self.stop(&s.job_id).await
    }
    pub async fn is_running(&self, job: &str) -> Result<bool> {
        let Some(e) = self.read(job)? else {
            return Ok(false);
        };
        Ok(e.state == "RUNNING"
            && self
                .inspect(&e)
                .await?
                .is_some_and(|v| v["State"]["Running"] == true))
    }
    pub async fn relay_stream(&self, job: &str, session: &str, now: i64) -> Result<(Pipe, String)> {
        let (id, token) = self.relay_upstream(job, session, now).await?;
        Ok((self.docker.open_stream(&id).await?, token))
    }
    pub async fn relay_upstream(
        &self,
        job: &str,
        session: &str,
        now: i64,
    ) -> Result<(String, String)> {
        let e = self.read(job)?.ok_or("UNKNOWN_NOTEBOOK")?;
        if e.spec.relay_session_id != session
            || !self
                .deadlines
                .lock()
                .map_err(|_| "PRIVATE_FILE_INVALID")?
                .get(job)
                .is_some_and(|t| *t > tokio::time::Instant::now())
        {
            return Err("INVALID_RELAY_AUTHORITY");
        }
        self.upstream(job, now).await
    }
    fn journal_jobs(&self) -> Result<Vec<String>> {
        let mut jobs = vec![];
        for entry in std::fs::read_dir(&self.root).map_err(|_| "PRIVATE_FILE_UNAVAILABLE")? {
            let path = entry.map_err(|_| "PRIVATE_FILE_INVALID")?.path();
            if path.extension().is_some_and(|e| e == "json") {
                let job = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .ok_or("PRIVATE_FILE_INVALID")?;
                if uuid(job) {
                    jobs.push(job.into());
                }
            }
        }
        Ok(jobs)
    }
    pub async fn stop_all(&self) -> Result<()> {
        let mut failed = false;
        for job in self.journal_jobs()? {
            match self.read(&job) {
                Ok(Some(e)) if e.state != "STOPPED" => {
                    if self.stop(&job).await.is_err() {
                        failed = true;
                    }
                }
                Err(_) => failed = true,
                _ => {}
            }
        }
        if failed {
            Err("NOTEBOOK_STOP_UNCONFIRMED")
        } else {
            Ok(())
        }
    }
    pub async fn enforce_deadlines(&self, now: i64) -> Result<Vec<NotebookResult>> {
        let mut results = vec![];
        let mut failed = false;
        for job in self.journal_jobs()? {
            match self.read(&job) {
                Ok(Some(e)) => {
                    let monotonic = self
                        .deadlines
                        .lock()
                        .map_err(|_| "PRIVATE_FILE_INVALID")?
                        .get(&job)
                        .is_some_and(|t| tokio::time::Instant::now() >= *t);
                    if e.state != "STOPPED" && (now >= e.spec.deadline || monotonic) {
                        match self.stop(&job).await {
                            Ok(r) => results.push(r),
                            Err(_) => failed = true,
                        }
                    }
                }
                Err(_) => failed = true,
                _ => {}
            }
        }
        if failed {
            Err("NOTEBOOK_STOP_UNCONFIRMED")
        } else {
            Ok(results)
        }
    }
    pub async fn upstream(&self, job: &str, now: i64) -> Result<(String, String)> {
        let e = self.read(job)?.ok_or("UNKNOWN_NOTEBOOK")?;
        if now < e.spec.not_before || now >= e.spec.deadline || e.state != "RUNNING" {
            return Err("NOTEBOOK_NOT_RUNNING");
        }
        let v = self.inspect(&e).await?.ok_or("NOTEBOOK_NOT_RUNNING")?;
        if v["State"]["Running"] != true {
            return Err("NOTEBOOK_NOT_RUNNING");
        }
        let container = v["Id"].as_str().ok_or("CONTAINER_OWNERSHIP_MISMATCH")?;
        Ok((container.to_owned(), e.token))
    }
}
fn stage_proxy(root: &Path) -> Result<String> {
    use sha2::{Digest, Sha256};
    use std::io::Write;
    #[cfg(test)]
    let bytes = b"\x7fELFtest-proxy".to_vec();
    #[cfg(not(test))]
    let bytes = {
        let path = std::env::current_exe().map_err(|_| "INVALID_PROXY_BINARY")?;
        let meta = std::fs::symlink_metadata(&path).map_err(|_| "INVALID_PROXY_BINARY")?;
        if !meta.is_file() || meta.len() > 32 * 1024 * 1024 {
            return Err("INVALID_PROXY_BINARY");
        }
        std::fs::read(path).map_err(|_| "INVALID_PROXY_BINARY")?
    };
    if !bytes.starts_with(b"\x7fELF") {
        return Err("INVALID_PROXY_BINARY");
    }
    let sha = format!("{:x}", Sha256::digest(&bytes));
    let dir = root.join("proxy-bin");
    let mut b = std::fs::DirBuilder::new();
    b.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        b.mode(0o700);
    }
    b.create(&dir).map_err(|_| "PRIVATE_FILE_UNAVAILABLE")?;
    let m = std::fs::symlink_metadata(&dir).map_err(|_| "PRIVATE_FILE_INVALID")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        if !m.is_dir()
            || m.permissions().mode() & 0o077 != 0
            || m.uid() != unsafe { libc::geteuid() }
        {
            return Err("PRIVATE_FILE_INVALID");
        }
    }
    let path = dir.join(format!("{sha}.elf"));
    if path.try_exists().map_err(|_| "INVALID_PROXY_BINARY")? {
        let mut o = std::fs::OpenOptions::new();
        o.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            o.custom_flags(libc::O_NOFOLLOW);
        }
        let mut f = o.open(&path).map_err(|_| "INVALID_PROXY_BINARY")?;
        let meta = f.metadata().map_err(|_| "INVALID_PROXY_BINARY")?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if meta.permissions().mode() & 0o777 != 0o555 {
                return Err("INVALID_PROXY_BINARY");
            }
        }
        if !meta.is_file() || meta.len() != bytes.len() as u64 {
            return Err("INVALID_PROXY_BINARY");
        }
        use std::io::Read;
        let mut old = Vec::new();
        f.read_to_end(&mut old)
            .map_err(|_| "INVALID_PROXY_BINARY")?;
        if old != bytes {
            return Err("INVALID_PROXY_BINARY");
        }
    } else {
        let mut o = std::fs::OpenOptions::new();
        o.create_new(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            o.mode(0o555).custom_flags(libc::O_NOFOLLOW);
        }
        let mut f = o.open(&path).map_err(|_| "PRIVATE_FILE_UNAVAILABLE")?;
        f.write_all(&bytes)
            .map_err(|_| "PRIVATE_FILE_UNAVAILABLE")?;
        // This fd was create_new: normalize only this new public-code file, never an existing path.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            f.set_permissions(std::fs::Permissions::from_mode(0o555))
                .map_err(|_| "PRIVATE_FILE_UNAVAILABLE")?;
        }
        f.sync_all().map_err(|_| "PRIVATE_FILE_UNAVAILABLE")?;
        std::fs::File::open(&dir)
            .and_then(|f| f.sync_all())
            .map_err(|_| "PRIVATE_FILE_UNAVAILABLE")?;
    }
    Ok(sha)
}
fn random() -> Result<[u8; 32]> {
    let mut v = [0; 32];
    getrandom::fill(&mut v).map_err(|_| "RANDOM_UNAVAILABLE")?;
    Ok(v)
}
fn result(s: &NotebookSpec, state: &str) -> NotebookResult {
    NotebookResult {
        state: state.into(),
        job_id: s.job_id.clone(),
        generation: s.job_id.clone(),
        error_code: None,
    }
}
#[cfg(test)]
#[allow(dead_code)]
pub fn sandbox_for_cpu_fixture(s: &NotebookSpec, token: &str, now: i64, proxy: &Path) -> Value {
    sandbox(s, token, now, proxy)
}
fn sandbox(s: &NotebookSpec, token: &str, now: i64, proxy: &Path) -> Value {
    json!({"Image":s.image_digest,"Labels":labels(s),"User":"1000:100","Entrypoint":["/usr/bin/timeout"],"Cmd":["--signal=TERM","--kill-after=10",format!("{}s",s.deadline-now),"/opt/conda/bin/jupyter","lab","--ServerApp.ip=0.0.0.0","--ServerApp.port=8888","--ServerApp.port_retries=0","--no-browser","--ServerApp.root_dir=/workspace","--ServerApp.allow_remote_access=True","--ServerApp.open_browser=False","--ServerApp.show_banner=False","--Application.log_level=WARN",format!("--ServerApp.base_url=/notebooks/{}/",s.job_id)],"Env":[format!("JUPYTER_TOKEN={token}"),"HOME=/tmp/home","JUPYTER_RUNTIME_DIR=/tmp/runtime","JUPYTER_CONFIG_DIR=/tmp/config","NVIDIA_REQUIRE_CUDA=cuda>=12.8","NVIDIA_DISABLE_REQUIRE=false","IPYTHONDIR=/tmp/ipython","MPLCONFIGDIR=/tmp/matplotlib"],"WorkingDir":"/workspace","ExposedPorts":{"8888/tcp":{}},"HostConfig":{"ReadonlyRootfs":true,"Privileged":false,"CapDrop":["ALL"],"SecurityOpt":["no-new-privileges:true"],"Memory":s.limits.memory_bytes,"MemorySwap":s.limits.memory_bytes,"NanoCpus":u64::from(s.limits.cpu_millis)*1000000,"PidsLimit":s.limits.pids,"ShmSize":67108864,"Tmpfs":{"/workspace":format!("rw,nosuid,nodev,mode=700,uid=1000,gid=100,size={}",s.limits.tmpfs_bytes),"/tmp":"rw,nosuid,nodev,mode=1777,size=134217728"},"Mounts":[{"Type":"bind","Source":proxy,"Target":"/usr/local/bin/cbgpu-node-proxy","ReadOnly":true,"BindOptions":{"Propagation":"rprivate"}}],"NetworkMode":"none","PublishAllPorts":false,"DeviceRequests":[{"Driver":"nvidia","DeviceIDs":[s.hardware_uuid],"Capabilities":[["gpu"]]}],"LogConfig":{"Type":"local","Config":{"max-size":"1m","max-file":"1","compress":"false"}},"RestartPolicy":{"Name":"no"}}})
}
