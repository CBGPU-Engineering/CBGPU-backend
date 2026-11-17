//! Launch/stop only application-owned native processes or labelled user-code containers.
use crate::resources::{Card, command};
use cbgpu_ai_store::{Result, Store, archive::extract_zip, fail, safe_relative, valid_id};
use serde_json::Value;
use std::{
    collections::HashSet,
    io::Read,
    path::{Path, PathBuf},
    process::Stdio,
};
#[async_trait::async_trait]
pub trait Task: Send {
    async fn poll(&mut self) -> Result<Option<i32>>;
    async fn own_pids(&mut self) -> Result<HashSet<u32>>;
    async fn stop(&mut self) -> Result<()>;
}
#[async_trait::async_trait]
pub trait Runner: Send + Sync {
    fn attached(&self) -> bool;
    async fn recover_owned(&self) -> Result<()>;
    async fn start(&self, store: &Store, job: &Value, gpu: &Card) -> Result<Box<dyn Task>>;
}
#[derive(Clone)]
pub struct NativeRunner {
    pub executable: PathBuf,
    pub python_image: String,
    pub python_memory: String,
    pub python_cpus: String,
    pub tool_dir: Option<PathBuf>,
    pub native_library_path: Option<String>,
    pub scoped_authorization: bool,
}
impl NativeRunner {
    pub fn new(executable: PathBuf, python_image: String) -> Result<Self> {
        if !executable.is_absolute() || !executable.is_file() {
            return Err(fail(
                "Native AI runner must be an explicit trusted absolute executable",
                503,
            ));
        }
        let canonical = executable.canonicalize()?;
        if canonical
            .file_name()
            .and_then(|v| v.to_str())
            .is_some_and(|name| {
                name.starts_with("python") || ["node", "bash", "sh", "perl", "ruby"].contains(&name)
            })
        {
            return Err(fail(
                "Interpreters are not native AI inference runners",
                503,
            ));
        }
        let mut magic = [0u8; 4];
        std::fs::File::open(&executable)?.read_exact(&mut magic)?;
        if magic != *b"\x7fELF" {
            return Err(fail(
                "AI inference runner must be a native ELF executable",
                503,
            ));
        }
        let native_library_path = std::env::var("CBGPU_AI_NATIVE_LIBRARY_PATH").ok();
        if native_library_path.as_ref().is_some_and(|value| {
            value.is_empty()
                || std::env::split_paths(value).any(|path| !path.is_absolute() || !path.is_dir())
        }) {
            return Err(fail(
                "Native library paths must be explicit existing absolute directories",
                503,
            ));
        }
        Ok(Self {
            executable: executable.canonicalize()?,
            python_image,
            python_memory: "32g".into(),
            python_cpus: "16".into(),
            tool_dir: None,
            native_library_path,
            scoped_authorization: crate::scoped_authorization_from_environment()?,
        })
    }
    fn tool(&self, name: &str) -> PathBuf {
        self.tool_dir
            .as_ref()
            .map(|p| p.join(name))
            .unwrap_or_else(|| PathBuf::from(name))
    }
}
pub fn prepare_workspace(store: &Store, job: &Value, data: &Value) -> Result<PathBuf> {
    let id = job["id"]
        .as_str()
        .filter(|v| valid_id(v))
        .ok_or_else(|| fail("Invalid job ID", 400))?;
    let workspace = store.workspace_root(id)?;
    std::fs::create_dir_all(workspace.join("output"))?;
    if !workspace.join(".prepared").exists() {
        if let Some(upload) = data["upload_id"].as_str() {
            if !valid_id(upload) {
                return Err(fail("Invalid upload ID", 400));
            }
            extract_zip(&store.root.join("uploads").join(upload), &workspace)?;
        }
        if let Some(code) = data["code"].as_str().filter(|s| !s.is_empty()) {
            use std::io::Write;
            let mut options = std::fs::OpenOptions::new();
            options.write(true).create(true).truncate(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
            }
            let mut file = options.open(workspace.join("main.py"))?;
            file.write_all(code.as_bytes())?;
        }
        let mut marker = std::fs::OpenOptions::new();
        marker.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            marker.mode(0o600);
        }
        marker.open(workspace.join(".prepared"))?;
    }
    let entry = safe_relative(data["entrypoint"].as_str().unwrap_or("main.py"))?;
    let path = workspace.join(entry);
    if !path.is_file()
        || path.is_symlink()
        || !path.canonicalize()?.starts_with(workspace.canonicalize()?)
    {
        return Err(fail("Entrypoint was not found in the ZIP", 400));
    }
    Ok(workspace)
}
#[async_trait::async_trait]
impl Runner for NativeRunner {
    fn attached(&self) -> bool {
        self.executable.is_file()
    }
    async fn recover_owned(&self) -> Result<()> {
        let names = command(
            &self.tool("docker"),
            &[
                "ps",
                "-a",
                "--filter",
                "label=cbgpu.ai.managed=true",
                "--format",
                "{{.Names}}",
            ]
            .map(str::to_owned),
            8,
        )
        .await?;
        for name in names.lines() {
            let Some(job) = name.strip_prefix("cbgpu-ai-").filter(|s| valid_id(s)) else {
                continue;
            };
            if labelled(&self.tool("docker"), name, job).await? {
                command(
                    &self.tool("docker"),
                    &["rm", "-f", name].map(str::to_owned),
                    8,
                )
                .await?;
            }
        }
        Ok(())
    }
    async fn start(&self, store: &Store, job: &Value, gpu: &Card) -> Result<Box<dyn Task>> {
        let id = job["id"]
            .as_str()
            .filter(|v| valid_id(v))
            .ok_or_else(|| fail("Invalid job ID", 400))?;
        let folder = store.root.join("jobs").join(id);
        let data = &job["input"];
        let mut args = vec!["--fsize=67108864".into(), "--".into()];
        let python = job["kind"] == "python";
        if python {
            let workspace = prepare_workspace(store, job, data)?;
            let name = format!("cbgpu-ai-{id}");
            args.push(self.tool("docker").to_string_lossy().to_string());
            args.extend(
                [
                    "run",
                    "--rm",
                    "--name",
                    &name,
                    "--label",
                    "cbgpu.ai.managed=true",
                    "--label",
                    &format!("cbgpu.ai.job={id}"),
                    "--gpus",
                    &format!("device={}", gpu.uuid),
                    "--network",
                    "none",
                    "--cap-drop",
                    "ALL",
                    "--security-opt",
                    "no-new-privileges",
                    "--read-only",
                    "--pids-limit",
                    "256",
                    "--memory",
                    &self.python_memory,
                    "--cpus",
                    &self.python_cpus,
                    "--shm-size",
                    "256m",
                    "--tmpfs",
                    "/tmp:rw,nosuid,nodev,size=256m",
                    "--user",
                    &format!("{}:{}", unsafe { libc::getuid() }, unsafe {
                        libc::getgid()
                    }),
                    "--mount",
                    &format!("type=bind,src={},dst=/workspace", workspace.display()),
                    "--workdir",
                    "/workspace",
                    "--env",
                    "HOME=/tmp",
                    "--env",
                    "PYTHONDONTWRITEBYTECODE=1",
                    "--env",
                    "OMP_NUM_THREADS=8",
                    &self.python_image,
                    "python",
                    "-u",
                    data["entrypoint"].as_str().unwrap_or("main.py"),
                ]
                .map(str::to_owned),
            );
        } else {
            args.extend([
                self.executable.to_string_lossy().to_string(),
                job["kind"].as_str().unwrap_or("").to_owned(),
                folder.to_string_lossy().to_string(),
                store.root.to_string_lossy().to_string(),
            ]);
        }
        let mut options = std::fs::OpenOptions::new();
        options.append(true).create(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
        }
        let log = options.open(folder.join("log.txt"))?;
        if !log.metadata()?.is_file() {
            return Err(fail("Invalid task log", 503));
        }
        let mut command = tokio::process::Command::new(self.tool("prlimit"));
        command
            .args(args)
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", &store.root)
            .env("LANG", "C.UTF-8")
            .env("CUDA_VISIBLE_DEVICES", &gpu.uuid)
            .env("OMP_NUM_THREADS", "8")
            .env("OPENBLAS_NUM_THREADS", "4")
            .env("MKL_NUM_THREADS", "4")
            .env("TOKENIZERS_PARALLELISM", "false")
            .env("HF_HUB_OFFLINE", "1")
            .env("HF_HUB_DISABLE_TELEMETRY", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::from(log.try_clone()?))
            .stderr(Stdio::from(log))
            .kill_on_drop(true);
        if !python {
            if let Some(libraries) = &self.native_library_path {
                command
                    .env("LD_LIBRARY_PATH", libraries)
                    .env("CBGPU_AI_NATIVE_LIBRARY_PATH", libraries);
            }
            for name in [
                "CBGPU_FFMPEG",
                "CBGPU_AI_DATA_ROOT",
                "CBGPU_AI_CPU_ONLY",
                "CBGPU_AI_MODELS_RUNNER",
                "CBGPU_AI_SEARCH_RUNNER",
            ] {
                if let Some(value) = std::env::var_os(name) {
                    command.env(name, value);
                }
            }
        }
        if !python
            && job["kind"] == "search"
            && std::env::var_os("CBGPU_AI_SEARCH_RUNNER").is_some()
        {
            for name in [
                "CBGPU_AI_CORPUS_POLICY_DATABASE_URL",
                "CBGPU_AI_MATERIALS_ROOT",
                "CBGPU_AI_PAPER_ORIGIN",
                "CBGPU_AI_CORPUS_OCR_IMAGE",
            ] {
                if let Some(value) = std::env::var_os(name) {
                    command.env(name, value);
                }
            }
        }
        if !python {
            // Canonical effective policy, rather than copying a stale raw flag.
            command.env(
                "CBGPU_ACCOUNT_V2",
                if self.scoped_authorization { "1" } else { "0" },
            );
        }
        if !python && let Ok(database) = std::env::var("CBGPU_AI_DATABASE_URL") {
            command.env("CBGPU_AI_DATABASE_URL", database);
        }
        #[cfg(unix)]
        command.process_group(0);
        let child = command
            .spawn()
            .map_err(|_| fail("Could not start native AI task", 503))?;
        let group = child
            .id()
            .ok_or_else(|| fail("Task process ID unavailable", 503))?;
        Ok(Box::new(NativeTask {
            child,
            group,
            job: id.into(),
            python,
            docker: self.tool("docker"),
        }))
    }
}
async fn labelled(docker: &Path, name: &str, job: &str) -> Result<bool> {
    let labels=command(docker,&["inspect","--format","{{index .Config.Labels \"cbgpu.ai.managed\"}} {{index .Config.Labels \"cbgpu.ai.job\"}}",name].map(str::to_owned),3).await?;
    Ok(labels.trim() == format!("true {job}"))
}
struct NativeTask {
    child: tokio::process::Child,
    group: u32,
    job: String,
    python: bool,
    docker: PathBuf,
}
#[async_trait::async_trait]
impl Task for NativeTask {
    async fn poll(&mut self) -> Result<Option<i32>> {
        Ok(self.child.try_wait()?.map(|s| s.code().unwrap_or(-1)))
    }
    async fn own_pids(&mut self) -> Result<HashSet<u32>> {
        if self.python {
            let name = format!("cbgpu-ai-{}", self.job);
            if !labelled(&self.docker, &name, &self.job).await? {
                return Err(fail("Container ownership cannot be verified", 503));
            }
            return Ok(command(
                &self.docker,
                &["top", &name, "-eo", "pid"].map(str::to_owned),
                3,
            )
            .await?
            .lines()
            .skip(1)
            .filter_map(|p| p.trim().parse().ok())
            .collect());
        }
        let mut pids = HashSet::from([self.group]);
        for item in std::fs::read_dir("/proc")? {
            let item = item?;
            let Ok(pid) = item.file_name().to_string_lossy().parse::<u32>() else {
                continue;
            };
            let Ok(stat) = std::fs::read_to_string(item.path().join("stat")) else {
                continue;
            };
            let Some(end) = stat.rfind(')') else {
                continue;
            };
            if stat[end + 1..]
                .split_whitespace()
                .nth(2)
                .and_then(|s| s.parse::<u32>().ok())
                == Some(self.group)
            {
                pids.insert(pid);
            }
        }
        Ok(pids)
    }
    async fn stop(&mut self) -> Result<()> {
        if self.python {
            let name = format!("cbgpu-ai-{}", self.job);
            if labelled(&self.docker, &name, &self.job)
                .await
                .unwrap_or(false)
            {
                let _ = command(
                    &self.docker,
                    &["stop", "-t", "3", &name].map(str::to_owned),
                    6,
                )
                .await;
            }
        }
        if self.child.try_wait()?.is_none() {
            if self.group > 1 {
                unsafe {
                    libc::kill(-(self.group as i32), libc::SIGTERM);
                }
            }
            if tokio::time::timeout(std::time::Duration::from_secs(4), self.child.wait())
                .await
                .is_err()
            {
                if self.group > 1 {
                    unsafe {
                        libc::kill(-(self.group as i32), libc::SIGKILL);
                    }
                }
                let _ = tokio::time::timeout(std::time::Duration::from_secs(3), self.child.wait())
                    .await;
            }
        }
        Ok(())
    }
}
