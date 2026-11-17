//! Reservation-first parallel scheduler. Only an attached native runner can execute jobs.
use crate::{
    Api,
    resources::{Card, Resources, Snapshot},
    runner::{Runner, Task, prepare_workspace},
};
use cbgpu_ai_store::{Result, atomic_json, fail, now, valid_id};
use serde_json::{Value, json};
use std::{collections::HashMap, path::Path, sync::Arc, time::Instant};
#[derive(Clone)]
pub struct Settings {
    pub workers: usize,
    pub admission_floor: u64,
    pub running_floor: u64,
    pub tick: std::time::Duration,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            workers: 4,
            admission_floor: 20 * 1024 * 1024 * 1024,
            running_floor: 10 * 1024 * 1024 * 1024,
            tick: std::time::Duration::from_secs(2),
        }
    }
}
#[derive(Default)]
struct State {
    leases: HashMap<String, String>,
    last_owner: Option<String>,
}
pub struct Scheduler {
    pub api: Arc<Api>,
    pub resources: Arc<dyn Resources>,
    pub runner: Arc<dyn Runner>,
    pub settings: Settings,
    state: tokio::sync::Mutex<State>,
    stop: tokio::sync::watch::Sender<bool>,
}
impl Scheduler {
    pub fn new(
        api: Arc<Api>,
        resources: Arc<dyn Resources>,
        runner: Arc<dyn Runner>,
        settings: Settings,
    ) -> Arc<Self> {
        let (stop, _) = tokio::sync::watch::channel(false);
        Arc::new(Self {
            api,
            resources,
            runner,
            settings,
            state: tokio::sync::Mutex::new(State::default()),
            stop,
        })
    }
    pub fn close(&self) {
        self.stop.send_replace(true);
    }
    pub async fn start(self: Arc<Self>) -> Result<tokio::task::JoinHandle<()>> {
        if !self.api.runtime_mode.local_execution() {
            return Err(fail("NO_EXECUTOR: local scheduler is disabled", 503));
        }
        if !self.runner.attached() {
            return Err(fail("Native inference runner is not attached", 503));
        }
        let lock = instance_lock(&self.api.store.root)?;
        self.runner.recover_owned().await?;
        sqlx::query("UPDATE ai_store.jobs SET state=CASE WHEN crashes>=2 THEN 'failed' ELSE 'paused' END,crashes=crashes+1,progress='Unexpected worker restart; inspect task log if repeated',updated=$1 WHERE state='running'").bind(now()).execute(&self.api.store.pool).await?;
        Ok(tokio::spawn(async move {
            let _lock = lock;
            let mut stop = self.stop.subscribe();
            loop {
                if *stop.borrow() {
                    break;
                }
                if let Err(_e) = self.tick().await {
                    *self.api.scheduler.write().await = json!({"status":"unavailable","available":false,"checked_at":now(),"gpus":[],"detail":"GPU or reservation status could not be verified. New tasks are paused.","reason":"gpu_or_queue_check_failed"});
                }
                tokio::select! {_ =tokio::time::sleep(self.settings.tick)=>{},_ =stop.changed()=>{}}
            }
            for _ in 0..300 {
                if self.api.running.read().await.is_empty() {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
            *self.api.scheduler.write().await = json!({"status":"stopped","available":false});
        }))
    }
    pub async fn tick(self: &Arc<Self>) -> Result<()> {
        if !self.api.runtime_mode.local_execution() {
            return Err(fail("NO_EXECUTOR: local scheduler is disabled", 503));
        }
        if *self.stop.borrow() {
            return Ok(());
        }
        if !self.runner.attached() {
            return Err(fail("Native inference runner is not attached", 503));
        }
        if self.api.store.data_root.is_some() {
            self.api.store.bulk_directory("workspaces")?;
        }
        let snapshot = self.resources.snapshot().await?;
        let active = self.api.running.read().await.len();
        let mut status = health(&snapshot, active, self.settings.workers);
        let leases = self
            .state
            .lock()
            .await
            .leases
            .keys()
            .cloned()
            .collect::<std::collections::HashSet<_>>();
        status["active_gpus"] = json!(
            snapshot
                .cards
                .iter()
                .filter(|c| leases.contains(&c.uuid))
                .map(|c| c.index + 1)
                .collect::<Vec<_>>()
        );
        *self.api.scheduler.write().await = status;
        if !self.runner.attached() {
            return Err(fail("Native inference runner is not attached", 503));
        }
        let rows:Vec<Value>=sqlx::query_scalar("SELECT to_jsonb(j) FROM ai_store.jobs j WHERE state IN('queued','paused') ORDER BY CASE WHEN kind='python' THEN 1 ELSE 0 END,updated,created").fetch_all(&self.api.store.pool).await?;
        let mut candidates = vec![];
        for job in rows {
            if !self
                .api
                .running
                .read()
                .await
                .contains(job["id"].as_str().unwrap_or(""))
                && self.document_ready(&job).await?
            {
                candidates.push(job);
            }
        }
        for gpu in snapshot.cards.iter().filter(|c| c.available) {
            let mut state = self.state.lock().await;
            if state.leases.len() >= self.settings.workers.clamp(1, 4)
                || state.leases.contains_key(&gpu.uuid)
            {
                continue;
            }
            let available = candidates
                .iter()
                .filter(|j| {
                    !state
                        .leases
                        .values()
                        .any(|id| Some(id.as_str()) == j["id"].as_str())
                })
                .collect::<Vec<_>>();
            let Some(first) = available.first() else {
                break;
            };
            let python = first["kind"] == "python";
            let same = available
                .into_iter()
                .filter(|j| (j["kind"] == "python") == python)
                .collect::<Vec<_>>();
            let job = (*same
                .iter()
                .find(|j| j["owner"].as_str() != state.last_owner.as_deref())
                .unwrap_or(&same[0]))
            .clone();
            let id = job["id"].as_str().unwrap_or("").to_owned();
            if !valid_id(&id) {
                return Err(fail("Invalid queued job identity", 503));
            }
            state.last_owner = job["owner"].as_str().map(str::to_owned);
            state.leases.insert(gpu.uuid.clone(), id.clone());
            self.api.running.write().await.insert(id);
            drop(state);
            let this = self.clone();
            let gpu = gpu.clone();
            tokio::spawn(async move {
                this.execute(job, gpu).await;
            });
        }
        self.cleanup().await?;
        Ok(())
    }
    async fn document_ready(&self, job: &Value) -> Result<bool> {
        if job["kind"] != "document" {
            return Ok(true);
        }
        let id = job["id"].as_str().unwrap_or("");
        let owner = job["owner"].as_str().unwrap_or("");
        let data = &job["input"];
        if data.get("extracted_text").is_some() {
            return Ok(true);
        }
        let upload = data["upload_id"].as_str().unwrap_or("");
        let status = self.api.preparation.status(owner, upload).await;
        match status {
            Ok(status) => {
                if status["state"] == "failed" {
                    self.finish(
                        id,
                        "failed",
                        status["error"]
                            .as_str()
                            .unwrap_or("Document extraction failed"),
                        0.,
                    )
                    .await?;
                    return Ok(false);
                }
                if status["state"] != "ready" {
                    match self.api.preparation.start(owner, upload).await {
                        Ok(_) => {}
                        Err(e) if e.status == 429 => {}
                        Err(e) => {
                            self.finish(id, "failed", &e.message, 0.).await?;
                        }
                    }
                    sqlx::query("UPDATE ai_store.jobs SET progress='문서 읽는 중 · 스캔 페이지 자동 OCR',updated=$1 WHERE id=$2 AND state IN('queued','paused')").bind(now()).bind(id).execute(&self.api.store.pool).await?;
                    return Ok(false);
                }
                let cached = self
                    .api
                    .preparation
                    .read(owner, upload)
                    .await?
                    .ok_or_else(|| fail("Document cache unavailable", 503))?;
                let result: cbgpu_ai_documents::Extracted =
                    serde_json::from_value(cached["result"].clone())
                        .map_err(|_| fail("Invalid document cache", 503))?;
                let mut input = data.clone();
                input["extracted_text"] = json!(
                    result
                        .chunks
                        .iter()
                        .map(|(_, t)| t.as_str())
                        .collect::<Vec<_>>()
                        .join("\n")
                );
                input["extraction_truncated"] = json!(result.truncated);
                input["ocr_pages"] = json!(result.ocr_page_numbers.len());
                let changed = sqlx::query(
                    "UPDATE ai_store.jobs SET input=$1 WHERE id=$2 AND state IN('queued','paused')",
                )
                .bind(&input)
                .bind(id)
                .execute(&self.api.store.pool)
                .await?
                .rows_affected();
                if changed > 0 {
                    atomic_json(
                        &self.api.store.root.join("jobs").join(id).join("input.json"),
                        &input,
                    )?;
                }
                Ok(changed > 0)
            }
            Err(e) => {
                if e.status != 429 {
                    self.finish(id, "failed", &e.message, 0.).await?;
                }
                Ok(false)
            }
        }
    }
    async fn execute(self: Arc<Self>, job: Value, gpu: Card) {
        let id = job["id"].as_str().unwrap_or("").to_owned();
        let start = Instant::now();
        let mut task: Option<Box<dyn Task>> = None;
        let result = self.run_job(job, &gpu, &mut task).await;
        let (state, message) = match result {
            Ok(outcome) => outcome,
            Err(error) => (
                match error.status {
                    401 | 403 => "cancelled",
                    503 => "paused",
                    _ => "failed",
                }
                .to_owned(),
                error.message,
            ),
        };
        if let Some(task) = task.as_mut() {
            let _ = task.stop().await;
        }
        let runtime = if task.is_some() {
            start.elapsed().as_secs_f64()
        } else {
            0.
        };
        let _ = self.finish(&id, &state, &message, runtime).await;
        self.state.lock().await.leases.remove(&gpu.uuid);
        self.api.running.write().await.remove(&id);
    }

    async fn run_job(
        &self,
        job: Value,
        gpu: &Card,
        task: &mut Option<Box<dyn Task>>,
    ) -> Result<(String, String)> {
        let id = job["id"].as_str().unwrap_or("");
        let owner = job["owner"].as_str().unwrap_or("");
        self.api.authorization.execution(owner).await?;
        if self.api.store.data_root.is_some() {
            self.api.store.bulk_directory("workspaces")?;
        }
        if job["kind"] == "python" {
            prepare_workspace(&self.api.store, &job, &job["input"])?;
        }
        let snapshot = self.resources.snapshot().await?;
        if !snapshot
            .cards
            .iter()
            .any(|card| card.uuid == gpu.uuid && card.available)
        {
            return Ok((
                "queued".into(),
                "GPU allocation changed before start".into(),
            ));
        }
        for path in self.storage_paths() {
            if self.resources.free_bytes(&path)? < self.settings.admission_floor {
                return Err(fail("AI storage is running low", 507));
            }
        }
        let claimed:Option<Value>=sqlx::query_scalar("UPDATE ai_store.jobs SET state='running',gpu=$1,attempts=attempts+1,started=$2,updated=$2,error=NULL,progress=$3 WHERE id=$4 AND state IN('queued','paused') RETURNING to_jsonb(jobs)")
            .bind(gpu.index as i32+1).bind(now()).bind(format!("Running on GPU {}",gpu.index+1)).bind(id).fetch_optional(&self.api.store.pool).await?;
        let Some(job) = claimed else {
            return Ok(("cancelled".into(), "Task no longer queued".into()));
        };
        let folder = self.api.store.root.join("jobs").join(id);
        let _ = std::fs::remove_file(folder.join("partial.json"));
        *task = Some(self.runner.start(&self.api.store, &job, gpu).await?);
        let start = Instant::now();
        let previous = job["runtime"].as_f64().unwrap_or(0.);
        let limit = if job["kind"] == "python" {
            job["input"]["minutes"].as_f64().unwrap_or(60.) * 60.
        } else {
            3600.
        };
        let mut last_permission = Instant::now();
        let mut last_workspace = Instant::now();
        loop {
            let task = task.as_mut().unwrap();
            if let Some(code) = task.poll().await? {
                if code != 0 {
                    return Ok((
                        "failed".into(),
                        "Task failed. See the task log for details.".into(),
                    ));
                }
                // A fast worker can finish between periodic checks. Revalidate
                // the effective module/resource policy before publishing success.
                self.api.authorization.execution(owner).await?;
                if job["kind"] == "python" {
                    atomic_json(
                        &folder.join("result.json"),
                        &json!({"text":"Python task completed. Download files saved in /workspace/output.","checkpoint_directory":"/workspace"}),
                    )?;
                }
                return Ok(if folder.join("result.json").is_file() {
                    ("succeeded".into(), "Completed".into())
                } else {
                    ("failed".into(), "Task finished without a result".into())
                });
            }
            if *self.stop.borrow() {
                return Ok(("paused".into(), "Service restart; waiting to resume".into()));
            }
            let state: Option<String> =
                sqlx::query_scalar("SELECT state FROM ai_store.jobs WHERE id=$1")
                    .bind(id)
                    .fetch_optional(&self.api.store.pool)
                    .await?;
            let free = self
                .storage_paths()
                .iter()
                .map(|path| self.resources.free_bytes(path))
                .collect::<Result<Vec<_>>>()?
                .into_iter()
                .min()
                .unwrap_or(0);
            let (reserved, foreign, resource_failed) = match self.resources.snapshot().await {
                Ok(snapshot) => match (
                    snapshot.cards.iter().find(|card| card.uuid == gpu.uuid),
                    task.own_pids().await,
                ) {
                    (Some(card), Ok(owned)) => (
                        card.reserved,
                        snapshot.processes.iter().any(|process| {
                            process.uuid == gpu.uuid && !owned.contains(&process.pid)
                        }),
                        false,
                    ),
                    _ => (false, false, true),
                },
                Err(_) => (false, false, true),
            };
            let permission_revoked = if last_permission.elapsed().as_secs() >= 5 {
                last_permission = Instant::now();
                self.api.authorization.execution(owner).await.is_err()
            } else {
                false
            };
            let short_work: i64 = if job["kind"] == "python" && start.elapsed().as_secs() > 60 {
                sqlx::query_scalar("SELECT count(*) FROM ai_store.jobs WHERE kind<>'python' AND state IN('queued','paused')").fetch_one(&self.api.store.pool).await?
            } else {
                0
            };
            let workspace_bytes =
                if last_workspace.elapsed().as_secs() >= 10 && start.elapsed().as_secs() > 30 {
                    last_workspace = Instant::now();
                    workspace_size(&self.api.store.workspace_root(id)?)?
                } else {
                    0
                };
            let monitor = Monitor {
                shutdown: *self.stop.borrow(),
                state: state.as_deref().unwrap_or("deleted"),
                free_bytes: free,
                floor: self.settings.running_floor,
                log_bytes: std::fs::metadata(folder.join("log.txt"))
                    .map(|m| m.len())
                    .unwrap_or(0),
                elapsed: previous + start.elapsed().as_secs_f64(),
                limit,
                reserved,
                foreign,
                resource_failed,
                permission_revoked,
                short_work: short_work > 0,
                workspace_bytes,
            };
            if let Some((state, message)) = decision(&monitor) {
                return Ok((state.into(), message.into()));
            }
            if let Ok(bytes) = std::fs::read(folder.join("progress.json"))
                && let Ok(progress) = serde_json::from_slice::<Value>(&bytes)
                && let Some(message) = progress["message"].as_str()
            {
                sqlx::query("UPDATE ai_store.jobs SET progress=$1,updated=$2 WHERE id=$3")
                    .bind(message.chars().take(200).collect::<String>())
                    .bind(now())
                    .bind(id)
                    .execute(&self.api.store.pool)
                    .await?;
            }
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        }
    }
    fn storage_paths(&self) -> Vec<std::path::PathBuf> {
        let mut paths = vec![self.api.store.root.clone()];
        if let Some(root) = &self.api.store.data_root {
            paths.push(root.clone());
        }
        paths
    }
    async fn finish(&self, id: &str, state: &str, message: &str, runtime: f64) -> Result<()> {
        sqlx::query("UPDATE ai_store.jobs SET state=CASE WHEN state IN('cancelled','deleted') THEN state ELSE $1 END,updated=$2,progress=$3,error=CASE WHEN $1='failed' THEN $3 ELSE NULL END,finished=CASE WHEN $1 IN('paused','queued') THEN NULL ELSE $2 END,runtime=runtime+$4 WHERE id=$5").bind(state).bind(now()).bind(message).bind(runtime.max(0.)).bind(id).execute(&self.api.store.pool).await?;
        if state == "succeeded" {
            self.persist_completion(id).await?;
        }
        Ok(())
    }
    async fn persist_completion(&self, id: &str) -> Result<()> {
        let path = self
            .api
            .store
            .root
            .join("jobs")
            .join(id)
            .join("result.json");
        if let Ok(bytes) = std::fs::read(path)
            && let Ok(result) = serde_json::from_slice::<Value>(&bytes)
            && let Some(content) = result["choices"][0]["message"]["content"].as_str()
        {
            sqlx::query(
                "UPDATE ai_store.chat_messages SET content=$1 WHERE job_id=$2 AND content=''",
            )
            .bind(content)
            .bind(id)
            .execute(&self.api.store.pool)
            .await?;
        }
        Ok(())
    }
    async fn cleanup(&self) -> Result<()> {
        let at = now();
        sqlx::query("DELETE FROM ai_store.downloads WHERE expires<$1 OR NOT EXISTS(SELECT 1 FROM ai_store.jobs j WHERE j.id=job_id)").bind(at).execute(&self.api.store.pool).await?;
        let old:Vec<(String,String)>=sqlx::query_as("SELECT id,owner FROM ai_store.jobs WHERE state='deleted' OR(state NOT IN('queued','running','paused')AND updated<$1)").bind(at-30.*86400.).fetch_all(&self.api.store.pool).await?;
        for (id, owner) in old {
            if !valid_id(&id) || self.api.running.read().await.contains(&id) {
                continue;
            }
            self.persist_completion(&id).await?;
            let mut tx = self.api.store.pool.begin().await?;
            sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
                .bind(format!("cbgpu:ai:owner:{owner}"))
                .execute(&mut *tx)
                .await?;
            let deleted:Option<Value>=sqlx::query_scalar("DELETE FROM ai_store.jobs WHERE id=$1 AND owner=$2 AND(state='deleted'OR(state NOT IN('queued','running','paused')AND updated<$3))RETURNING input").bind(&id).bind(&owner).bind(at-30.*86400.).fetch_optional(&mut *tx).await?;
            let mut removed_upload: Option<String> = None;
            if let Some(input) = &deleted
                && let Some(upload) = input["upload_id"].as_str()
            {
                removed_upload=sqlx::query_scalar("DELETE FROM ai_store.uploads u WHERE id=$1 AND owner=$2 AND job_id=$3 AND NOT EXISTS(SELECT 1 FROM ai_store.ai_project_files f WHERE f.upload=u.id)RETURNING id").bind(upload).bind(&owner).bind(&id).fetch_optional(&mut *tx).await?;
            }
            tx.commit().await?;
            if deleted.is_some() {
                if let Some(upload) = removed_upload {
                    self.api.store.remove_upload_files(&upload)?;
                }
                self.api.store.remove_job_files(&id)?;
            }
        }
        let old: Vec<(String, String)> =
            sqlx::query_as("SELECT id,owner FROM ai_store.uploads WHERE created<$1")
                .bind(at - 14. * 86400.)
                .fetch_all(&self.api.store.pool)
                .await?;
        for (upload, owner) in old {
            let mut tx = self.api.store.pool.begin().await?;
            sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
                .bind(format!("cbgpu:ai:owner:{owner}"))
                .execute(&mut *tx)
                .await?;
            let removed:Option<String>=sqlx::query_scalar("DELETE FROM ai_store.uploads u WHERE id=$1 AND owner=$2 AND created<$3 AND NOT EXISTS(SELECT 1 FROM ai_store.ai_project_files f WHERE f.upload=u.id)AND(job_id IS NULL OR NOT EXISTS(SELECT 1 FROM ai_store.jobs j WHERE j.id=u.job_id AND j.state IN('queued','running','paused')))RETURNING id").bind(&upload).bind(&owner).bind(at-14.*86400.).fetch_optional(&mut *tx).await?;
            tx.commit().await?;
            if let Some(upload) = removed {
                self.api.store.remove_upload_files(&upload)?;
            }
        }
        Ok(())
    }
}
fn health(snapshot: &Snapshot, active: usize, workers: usize) -> Value {
    json!({"status":"ok","available":true,"checked_at":now(),"workers":workers.clamp(1,4),"active_jobs":active,"gpus":snapshot.cards.iter().map(|c|json!({"id":c.index+1,"name":c.name,"reserved":c.reserved,"available":c.available})).collect::<Vec<_>>()})
}
pub struct Monitor<'a> {
    pub shutdown: bool,
    pub state: &'a str,
    pub free_bytes: u64,
    pub floor: u64,
    pub log_bytes: u64,
    pub elapsed: f64,
    pub limit: f64,
    pub reserved: bool,
    pub foreign: bool,
    pub resource_failed: bool,
    pub permission_revoked: bool,
    pub short_work: bool,
    pub workspace_bytes: u64,
}
pub fn decision(m: &Monitor<'_>) -> Option<(&'static str, &'static str)> {
    if m.state == "deleted" {
        return Some(("deleted", "Cancelled by owner"));
    }
    if m.state == "cancelled" {
        return Some(("cancelled", "Cancelled by owner"));
    }
    if m.shutdown {
        return Some(("paused", "Service restart; waiting to resume"));
    }
    if m.free_bytes < m.floor {
        return Some(("failed", "AI storage is running low"));
    }
    if m.log_bytes > 60 * 1024 * 1024 {
        return Some(("failed", "Task log exceeded 60 MB"));
    }
    if m.elapsed > m.limit {
        return Some(("failed", "Task runtime limit reached"));
    }
    if m.resource_failed {
        return Some(("paused", "GPU reservation check unavailable; paused safely"));
    }
    if m.reserved || m.foreign {
        return Some((
            "paused",
            "GPU reservation or notebook workload takes priority",
        ));
    }
    if m.permission_revoked {
        return Some(("cancelled", "Account access revoked"));
    }
    if m.short_work {
        return Some((
            "paused",
            "Short AI task takes priority; workspace is preserved",
        ));
    }
    if m.workspace_bytes > 2 * 1024 * 1024 * 1024 {
        return Some(("failed", "Workspace exceeded 2 GB"));
    }
    None
}
fn workspace_size(root: &Path) -> Result<u64> {
    if !root.exists() {
        return Ok(0);
    }
    let mut stack = vec![root.to_owned()];
    let mut total = 0u64;
    let mut count = 0;
    while let Some(dir) = stack.pop() {
        for e in std::fs::read_dir(dir)? {
            let e = e?;
            count += 1;
            if count > 100000 {
                return Err(fail("Workspace contains too many files", 413));
            }
            let ty = e.file_type()?;
            if ty.is_dir() {
                stack.push(e.path());
            } else if ty.is_file() {
                total = total.saturating_add(e.metadata()?.len());
                if total > 2 * 1024 * 1024 * 1024 {
                    return Ok(total);
                }
            }
        }
    }
    Ok(total)
}
#[cfg(unix)]
fn instance_lock(root: &Path) -> Result<std::fs::File> {
    use std::os::{fd::AsRawFd, unix::fs::OpenOptionsExt};
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(root.join(".native-scheduler.lock"))?;
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return Err(fail("Another native AI scheduler is already attached", 409));
    }
    Ok(file)
}
#[cfg(not(unix))]
fn instance_lock(_: &Path) -> Result<std::fs::File> {
    Err(fail("Scheduler process locking unavailable", 503))
}
#[cfg(test)]
mod tests {
    use super::*;
    fn sample() -> Monitor<'static> {
        Monitor {
            shutdown: false,
            state: "running",
            free_bytes: 100,
            floor: 10,
            log_bytes: 0,
            elapsed: 1.,
            limit: 100.,
            reserved: false,
            foreign: false,
            resource_failed: false,
            permission_revoked: false,
            short_work: false,
            workspace_bytes: 0,
        }
    }
    #[test]
    fn reservation_failure_or_foreign_workload_preempts_only_owned_task() {
        let mut m = sample();
        assert!(decision(&m).is_none());
        m.reserved = true;
        assert_eq!(decision(&m).unwrap().0, "paused");
        m.reserved = false;
        m.foreign = true;
        assert_eq!(decision(&m).unwrap().0, "paused");
        m.foreign = false;
        m.resource_failed = true;
        assert_eq!(decision(&m).unwrap().0, "paused");
    }
    #[test]
    fn cancellation_quota_timeout_restart_and_permission_are_bounded() {
        let mut m = sample();
        m.state = "deleted";
        m.shutdown = true;
        assert_eq!(decision(&m).unwrap().0, "deleted");
        m.state = "running";
        assert_eq!(decision(&m).unwrap().0, "paused");
        m.shutdown = false;
        m.permission_revoked = true;
        assert_eq!(decision(&m).unwrap().0, "cancelled");
        m.permission_revoked = false;
        m.elapsed = 101.;
        assert_eq!(decision(&m).unwrap().0, "failed");
        m.elapsed = 1.;
        m.log_bytes = 61 * 1024 * 1024;
        assert_eq!(decision(&m).unwrap().0, "failed");
    }
    #[test]
    fn only_one_native_scheduler_per_state_directory() {
        let t = tempfile::tempdir().unwrap();
        let lock = instance_lock(t.path()).unwrap();
        assert!(instance_lock(t.path()).is_err());
        drop(lock);
        assert!(instance_lock(t.path()).is_ok());
    }
}
