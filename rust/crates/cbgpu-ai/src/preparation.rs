use cbgpu_ai_store::{Result, Store, atomic_json, fail};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    fs::OpenOptions,
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::{
    io::AsyncReadExt,
    sync::{Mutex, Semaphore},
};
#[derive(Clone)]
pub struct Preparation {
    local_execution: bool,
    tool_dir: Option<PathBuf>,
    store: Store,
    image: String,
    active: Arc<Mutex<HashMap<String, Value>>>,
    slots: Arc<Semaphore>,
}
pub struct Spool {
    pub path: PathBuf,
}
impl Spool {
    pub fn new(path: PathBuf) -> Result<Self> {
        let mut o = OpenOptions::new();
        o.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            o.mode(0o600);
        }
        o.open(&path)?;
        Ok(Self { path })
    }
}
impl Drop for Spool {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}
impl Preparation {
    pub fn new(store: Store, image: String) -> Self {
        Self {
            local_execution: true,
            tool_dir: None,
            store,
            image,
            active: Arc::new(Mutex::new(HashMap::new())),
            slots: Arc::new(Semaphore::new(2)),
        }
    }
    pub fn with_local_execution(mut self, enabled: bool) -> Self {
        self.local_execution = enabled;
        self
    }
    /// Trusted process configuration only; never populated from uploaded/job data.
    pub fn with_tool_directory(mut self, directory: PathBuf) -> Result<Self> {
        if !directory.is_absolute() || !directory.is_dir() {
            return Err(fail("Invalid native tool directory", 503));
        }
        self.tool_dir = Some(directory);
        Ok(self)
    }
    fn docker(&self) -> PathBuf {
        self.tool_dir
            .as_ref()
            .map(|p| p.join("docker"))
            .unwrap_or_else(|| "docker".into())
    }
    fn cache(&self, id: &str) -> PathBuf {
        self.store.root.join("extracted").join(format!("{id}.json"))
    }
    pub(crate) async fn read(&self, owner: &str, id: &str) -> Result<Option<Value>> {
        self.store.get_upload(owner, id).await?;
        let value = std::fs::read(self.cache(id))
            .ok()
            .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
            .filter(|v| v["version"] == 1);
        Ok(value)
    }
    pub async fn status(&self, owner: &str, id: &str) -> Result<Value> {
        let upload = self.store.get_upload(owner, id).await?;
        let active = self.active.lock().await.get(id).cloned();
        let mut value = active
            .clone()
            .or(self.read(owner, id).await?)
            .unwrap_or(json!({"state":"new"}));
        if value["state"] == "processing" && active.is_none() {
            value = json!({"state":"failed","error":"문서 처리가 중단됐어요. 다시 시도해 주세요."});
        }
        value["id"] = json!(id);
        value["name"] = upload["name"].clone();
        let keys = [
            "id",
            "name",
            "state",
            "error",
            "progress",
            "pages",
            "ocr_pages",
            "weak_pages",
            "empty_pages",
            "truncated",
        ];
        value
            .as_object_mut()
            .unwrap()
            .retain(|k, _| keys.contains(&k.as_str()));
        Ok(value)
    }
    pub async fn start(&self, owner: &str, id: &str) -> Result<Value> {
        let upload = self.store.get_upload(owner, id).await?;
        let name = upload["name"].as_str().unwrap_or("");
        let suffix = format!(
            ".{}",
            Path::new(name)
                .extension()
                .and_then(|v| v.to_str())
                .unwrap_or("")
                .to_lowercase()
        );
        let visual =
            [".pdf", ".png", ".jpg", ".jpeg", ".webp", ".tif", ".tiff"].contains(&suffix.as_str());
        let text =
            [".txt", ".md", ".csv", ".json", ".tex", ".py", ".log"].contains(&suffix.as_str());
        let max = if suffix == ".pdf" {
            200 * 1024 * 1024
        } else {
            8 * 1024 * 1024
        };
        if (!visual && !text) || upload["size"].as_i64().unwrap_or(i64::MAX) > max {
            return Err(fail(
                "8MB 이하의 PDF, 이미지, CSV 또는 텍스트 파일을 올려 주세요.",
                400,
            ));
        }
        let old = self.status(owner, id).await?;
        if ["ready", "processing"].contains(&old["state"].as_str().unwrap_or("")) {
            return Ok(old);
        }
        if !self.local_execution {
            return Err(fail(
                "NO_EXECUTOR: document preparation is unavailable on this control plane",
                503,
            ));
        }
        let slot = self
            .slots
            .clone()
            .try_acquire_owned()
            .map_err(|_| fail("다른 파일을 읽고 있어요. 잠시 후 다시 시도해 주세요.", 429))?;
        std::fs::create_dir_all(self.store.root.join("extracted"))?;
        let initial = json!({"state":"processing","progress":0,"version":1});
        {
            let mut active = self.active.lock().await;
            if active.contains_key(id) {
                return self.status_unlocked(&upload, id, active.get(id).unwrap());
            }
            active.insert(id.into(), initial.clone());
        }
        if let Err(error) = atomic_json(&self.cache(id), &initial) {
            self.active.lock().await.remove(id);
            return Err(error);
        }

        let response = self.status_unlocked(&upload, id, &initial)?;
        let service = self.clone();
        let owner = owner.to_owned();
        let id = id.to_owned();
        tokio::spawn(async move {
            let result = service.run(&id, &suffix, visual).await.and_then(|value| {
                if value.chunks.iter().any(|(_, text)| !text.trim().is_empty()) {
                    Ok(value)
                } else {
                    Err(fail("읽을 수 있는 텍스트가 없어요.", 400))
                }
            });
            let value = match result {
                Ok(result) => {
                    json!({"version":1,"state":"ready","pages":result.pages,"progress":result.pages,"ocr_pages":result.ocr_page_numbers.len(),"weak_pages":result.weak_pages,"empty_pages":result.empty_pages,"truncated":result.truncated,"result":result})
                }
                Err(e) => {
                    json!({"version":1,"state":"failed","error":e.message.chars().take(300).collect::<String>()})
                }
            };
            if service.store.get_upload(&owner, &id).await.is_ok() {
                let _ = atomic_json(&service.cache(&id), &value);
            }
            service.active.lock().await.remove(&id);
            drop(slot);
        });
        Ok(response)
    }
    fn status_unlocked(&self, upload: &Value, id: &str, current: &Value) -> Result<Value> {
        let mut value = current.clone();
        value["id"] = json!(id);
        value["name"] = upload["name"].clone();
        Ok(value)
    }
    async fn run(
        &self,
        id: &str,
        suffix: &str,
        visual: bool,
    ) -> Result<cbgpu_ai_documents::Extracted> {
        if !self.local_execution {
            return Err(fail(
                "NO_EXECUTOR: local document extraction is disabled",
                503,
            ));
        }
        let source = self.store.root.join("uploads").join(id).canonicalize()?;
        if !visual {
            let suffix = suffix.to_owned();
            return tokio::task::spawn_blocking(move || {
                cbgpu_ai_documents::extract_text(&source, &suffix)
            })
            .await
            .map_err(|_| fail("Document extraction failed", 503))?
            .map_err(|e| fail(&e, 400));
        }
        let name = format!("cbgpu-ocr-{}", uuid::Uuid::new_v4());
        let mut command = tokio::process::Command::new(self.docker());
        let uid = unsafe { libc_uid() };
        command
            .args([
                "run",
                "--rm",
                "--name",
                &name,
                "--network",
                "none",
                "--label",
                "cbgpu.ocr.managed=true",
                "--cap-drop",
                "ALL",
                "--security-opt",
                "no-new-privileges",
                "--read-only",
                "--pids-limit",
                "64",
                "--cpus",
                "2",
                "--memory",
                "1g",
                "--tmpfs",
                "/tmp:rw,nosuid,nodev,size=128m",
                "--user",
                &uid,
                "--mount",
                &format!("type=bind,src={},dst=/input,readonly", source.display()),
                &self.image,
                "/input",
                suffix,
            ])
            .kill_on_drop(true)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null());
        let mut child = command
            .spawn()
            .map_err(|_| fail("Native OCR worker unavailable", 503))?;
        let output = child
            .stdout
            .take()
            .ok_or_else(|| fail("OCR output unavailable", 503))?;
        let active = self.active.clone();
        let id = id.to_owned();
        let task = async {
            let mut output = output;
            let mut pending = vec![];
            let mut chunk = [0u8; 8192];
            let mut result = None;
            loop {
                let n = output.read(&mut chunk).await?;
                if n == 0 {
                    break;
                }
                pending.extend_from_slice(&chunk[..n]);
                if pending.len() > 3_000_000 {
                    return Err(fail("문서가 너무 커요. 파일을 나누어 올려 주세요.", 413));
                }
                while let Some(end) = pending.iter().position(|b| *b == b'\n') {
                    let line = pending.drain(..=end).collect::<Vec<_>>();
                    let value: Value = serde_json::from_slice(&line)?;
                    if let Some(error) = value["error"].as_str() {
                        return Err(fail(error, 400));
                    }
                    if let Some(value) = value.get("result") {
                        result = Some(
                            serde_json::from_value(value.clone())
                                .map_err(|_| fail("Invalid OCR result", 503))?,
                        );
                    } else if let Some(current) = active.lock().await.get_mut(&id) {
                        for key in ["progress", "pages", "ocr_pages"] {
                            if let Some(value) = value.get(key) {
                                current[key] = value.clone();
                            }
                        }
                    }
                }
            }
            let status = child.wait().await?;
            if !status.success() {
                return Err(fail(
                    "문서 처리 시간이 초과됐어요. 파일을 나누어 다시 올려 주세요.",
                    400,
                ));
            }
            result.ok_or_else(|| fail("OCR result unavailable", 503))
        };
        let result = tokio::time::timeout(std::time::Duration::from_secs(240), task)
            .await
            .map_err(|_| fail("Document extraction timed out", 400))
            .and_then(|r| r);
        let _ = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            tokio::process::Command::new(self.docker())
                .args(["rm", "-f", &name])
                .kill_on_drop(true)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status(),
        )
        .await;
        result
    }
    pub async fn text(&self, owner: &str, id: &str) -> Result<String> {
        let value = self
            .read(owner, id)
            .await?
            .filter(|v| v["state"] == "ready")
            .ok_or_else(|| fail("문서 처리가 끝난 뒤 확인할 수 있어요.", 409))?;
        let result: cbgpu_ai_documents::Extracted = serde_json::from_value(value["result"].clone())
            .map_err(|_| fail("Invalid extraction cache", 503))?;
        Ok(result
            .chunks
            .into_iter()
            .map(|(p, t)| p.map(|p| format!("[{p}쪽]\n{t}")).unwrap_or(t))
            .collect::<Vec<_>>()
            .join("\n\n"))
    }
    pub async fn evidence(&self, owner: &str, id: &str, question: &str) -> Result<Vec<Value>> {
        let upload = self.store.get_upload(owner, id).await?;
        let cached = self.read(owner, id).await?;
        let value = if let Some(value) = cached.filter(|v| v["state"] == "ready") {
            value
        } else {
            self.start(owner, id).await?;
            return Err(fail(
                "첨부 문서를 읽고 있어요. 완료 후 다시 보내 주세요.",
                409,
            ));
        };
        let result: cbgpu_ai_documents::Extracted = serde_json::from_value(value["result"].clone())
            .map_err(|_| fail("Invalid extraction cache", 503))?;
        Ok(cbgpu_ai_documents::evidence(
            upload["name"].as_str().unwrap_or("upload"),
            question,
            &result,
            true,
        ))
    }
}
#[cfg(unix)]
unsafe fn libc_uid() -> String {
    unsafe extern "C" {
        fn getuid() -> u32;
        fn getgid() -> u32;
    }
    format!("{}:{}", unsafe { getuid() }, unsafe { getgid() })
}
#[cfg(not(unix))]
unsafe fn libc_uid() -> String {
    "1000:1000".into()
}
