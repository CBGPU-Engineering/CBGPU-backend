//! Private PostgreSQL AI persistence. No SQLite writes, proxy or Python subprocesses.
//! Callers must supply the identity verified by the existing Auth/API-key boundary.
use base64::Engine;
use rand::RngCore;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Postgres, Transaction};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::{Component, Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};
use uuid::Uuid;
pub mod archive;
pub mod conversations;
pub mod importer;
pub mod validation;
pub const LIVE: [&str; 3] = ["queued", "running", "paused"];
pub const KINDS: [&str; 6] = [
    "chat",
    "transcribe",
    "embed",
    "search",
    "document",
    "python",
];
pub const MAX_UPLOAD: u64 = 200 * 1024 * 1024;
#[derive(Debug)]
pub struct Failure {
    pub message: String,
    pub status: u16,
}
impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}
impl std::error::Error for Failure {}
impl From<sqlx::Error> for Failure {
    fn from(_: sqlx::Error) -> Self {
        fail("AI persistence unavailable", 503)
    }
}
impl From<std::io::Error> for Failure {
    fn from(_: std::io::Error) -> Self {
        fail("AI storage unavailable", 503)
    }
}
impl From<serde_json::Error> for Failure {
    fn from(_: serde_json::Error) -> Self {
        fail("Invalid persisted AI data", 503)
    }
}
pub type Result<T> = std::result::Result<T, Failure>;
pub fn fail(message: &str, status: u16) -> Failure {
    Failure {
        message: message.into(),
        status,
    }
}
pub fn valid_id(v: &str) -> bool {
    Uuid::parse_str(v).is_ok_and(|id| id.to_string() == v)
}
pub fn safe_relative(v: &str) -> Result<PathBuf> {
    if v.is_empty()
        || v.contains(['\\', '\0'])
        || v.starts_with('/')
        || v.split('/').any(|p| p == "..")
        || Path::new(v).components().any(|p| {
            matches!(
                p,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(fail("Invalid file path", 400));
    }
    Ok(PathBuf::from(v))
}
pub fn now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}
fn id() -> String {
    Uuid::new_v4().to_string()
}
fn private_dir(p: &Path) -> Result<()> {
    fs::create_dir_all(p)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(p, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}
fn create_file(p: &Path) -> Result<File> {
    let mut o = OpenOptions::new();
    o.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    Ok(o.open(p)?)
}
pub fn atomic_json(path: &Path, value: &Value) -> Result<()> {
    let temp = path.with_extension(format!("{}.tmp", id()));
    let mut file = create_file(&temp)?;
    let result = (|| {
        file.write_all(&serde_json::to_vec(value)?)?;
        file.sync_all()?;
        fs::rename(&temp, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(temp);
    }
    result
}
#[derive(Clone)]
pub struct Limits {
    pub daily_uploads: i64,
    pub owner_upload_bytes: u64,
    pub upload_bytes: u64,
    pub free_floor: u64,
    pub require_data_mount: bool,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            daily_uploads: 30,
            owner_upload_bytes: 2 * 1024 * 1024 * 1024,
            upload_bytes: 200 * 1024 * 1024,
            free_floor: 20 * 1024 * 1024 * 1024,
            require_data_mount: false,
        }
    }
}
#[derive(Clone)]
pub struct Store {
    pub pool: PgPool,
    pub root: PathBuf,
    pub data_root: Option<PathBuf>,
    pub limits: Limits,
}
impl Store {
    pub fn new(
        pool: PgPool,
        root: PathBuf,
        data_root: Option<PathBuf>,
        limits: Limits,
    ) -> Result<Self> {
        private_dir(&root)?;
        let root = root.canonicalize()?;
        for n in ["jobs", "uploads", "models"] {
            private_dir(&root.join(n))?;
        }
        let data_root = if let Some(p) = data_root {
            private_dir(&p)?;
            Some(p.canonicalize()?)
        } else {
            None
        };
        Ok(Self {
            pool,
            root,
            data_root,
            limits,
        })
    }
    pub async fn migrate(&self) -> Result<()> {
        sqlx::migrate!("./migrations")
            .set_ignore_missing(true)
            .run(&self.pool)
            .await
            .map_err(|_| fail("AI schema migration failed", 503))?;
        Ok(())
    }
    pub(crate) async fn owner_lock(&self, owner: &str) -> Result<Transaction<'_, Postgres>> {
        if !valid_id(owner) {
            return Err(fail("Invalid owner", 401));
        }
        let mut tx = self.pool.begin().await?;
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
            .bind(format!("cbgpu:ai:owner:{owner}"))
            .execute(&mut *tx)
            .await?;
        Ok(tx)
    }
    pub fn bulk_directory(&self, name: &str) -> Result<PathBuf> {
        if !["uploads", "workspaces"].contains(&name) {
            return Err(fail("Invalid bulk directory", 400));
        }
        if self.data_root.is_some()
            && self.limits.require_data_mount
            && !fs::read_to_string("/proc/self/mountinfo")
                .unwrap_or_default()
                .lines()
                .any(|l| l.split_whitespace().nth(4) == Some("/srv/cbgpu-data"))
        {
            return Err(fail(
                "추가 데이터 SSD가 연결되지 않았어요. 저장 작업을 잠시 중단합니다.",
                503,
            ));
        }
        let p = self.data_root.as_ref().unwrap_or(&self.root).join(name);
        private_dir(&p)?;
        Ok(p)
    }
    pub async fn upload<R: Read>(&self, owner: &str, name: &str, mut reader: R) -> Result<Value> {
        let mut tx = self.owner_lock(owner).await?;
        let daily: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM ai_store.uploads WHERE owner=$1 AND created>$2",
        )
        .bind(owner)
        .bind(now() - 86400.)
        .fetch_one(&mut *tx)
        .await?;
        if daily >= self.limits.daily_uploads {
            return Err(fail("Upload limit reached today", 429));
        }
        let owned: i64 = sqlx::query_scalar(
            "SELECT coalesce(sum(size),0)::bigint FROM ai_store.uploads WHERE owner=$1",
        )
        .bind(owner)
        .fetch_one(&mut *tx)
        .await?;
        let upload = id();
        let dir = self.bulk_directory("uploads")?;
        let target = dir.join(&upload);
        let link = self.root.join("uploads").join(&upload);
        let name = name
            .replace('\\', "/")
            .rsplit('/')
            .next()
            .unwrap_or("upload")
            .chars()
            .take(180)
            .collect::<String>();
        let name = if name.is_empty() {
            "upload".into()
        } else {
            name
        };
        let mut size = 0u64;
        let written: Result<()> = (|| {
            let mut f = create_file(&target)?;
            let mut chunk = [0u8; 65536];
            loop {
                let n = reader.read(&mut chunk)?;
                if n == 0 {
                    break;
                }
                size += n as u64;
                if size > self.limits.upload_bytes {
                    return Err(fail("Upload limit is 200 MB", 413));
                }
                if owned as u64 + size > self.limits.owner_upload_bytes {
                    return Err(fail(
                        "Your AI uploads exceed 2 GB. Delete old tasks first.",
                        413,
                    ));
                }
                if available(&dir)? < self.limits.free_floor {
                    return Err(fail(
                        "AI storage is running low. Try again after old results are removed.",
                        507,
                    ));
                }
                f.write_all(&chunk[..n])?;
            }
            if size == 0 {
                return Err(fail("The file is empty", 400));
            }
            f.sync_all()?;
            #[cfg(unix)]
            if self.data_root.is_some() {
                std::os::unix::fs::symlink(&target, &link)?;
            }
            Ok(())
        })();
        if let Err(e) = written {
            let _ = fs::remove_file(&target);
            if link != target {
                let _ = fs::remove_file(link);
            }
            return Err(e);
        }
        let persisted: Result<()> = async {
            sqlx::query(
                "INSERT INTO ai_store.uploads(id,owner,name,size,created)VALUES($1,$2,$3,$4,$5)",
            )
            .bind(&upload)
            .bind(owner)
            .bind(&name)
            .bind(size as i64)
            .bind(now())
            .execute(&mut *tx)
            .await?;
            tx.commit().await?;
            Ok(())
        }
        .await;
        if let Err(e) = persisted {
            let _ = fs::remove_file(target);
            if link.exists() && self.data_root.is_some() {
                let _ = fs::remove_file(link);
            }
            return Err(e);
        }
        Ok(json!({"id":upload,"name":name,"size":size}))
    }
    pub async fn get_upload(&self, owner: &str, upload: &str) -> Result<Value> {
        if !valid_id(upload) {
            return Err(fail("Upload not found", 404));
        }
        let row: Option<Value> = sqlx::query_scalar(
            "SELECT to_jsonb(u) FROM ai_store.uploads u WHERE id=$1 AND owner=$2",
        )
        .bind(upload)
        .bind(owner)
        .fetch_optional(&self.pool)
        .await?;
        let link = self.root.join("uploads").join(upload);
        if !link.is_file()
            || (link.is_symlink()
                && self.data_root.as_ref().is_none_or(|root| {
                    link.canonicalize().ok() != Some(root.join("uploads").join(upload))
                }))
        {
            return Err(fail("Upload not found", 404));
        }
        row.ok_or_else(|| fail("Upload not found", 404))
    }
    pub fn remove_upload_files(&self, upload: &str) -> Result<()> {
        if !valid_id(upload) {
            return Err(fail("Invalid upload ID", 400));
        }
        let extracted = self.root.join("extracted").join(format!("{upload}.json"));
        remove_optional(&extracted)?;
        let link = self.root.join("uploads").join(upload);
        if link.is_symlink()
            && let Some(root) = &self.data_root
            && link.canonicalize().ok() == Some(root.join("uploads").join(upload))
        {
            remove_optional(&root.join("uploads").join(upload))?;
        }
        remove_optional(&link)?;
        Ok(())
    }
    pub async fn delete_upload(&self, owner: &str, upload: &str) -> Result<()> {
        let mut tx = self.owner_lock(owner).await?;
        let row: Option<Option<String>> = sqlx::query_scalar(
            "SELECT job_id FROM ai_store.uploads WHERE id=$1 AND owner=$2 FOR UPDATE",
        )
        .bind(upload)
        .bind(owner)
        .fetch_optional(&mut *tx)
        .await?;
        let job = row.ok_or_else(|| fail("Upload not found", 404))?;
        let used: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM ai_store.ai_project_files WHERE upload=$1)",
        )
        .bind(upload)
        .fetch_one(&mut *tx)
        .await?;
        if job.is_some() || used {
            return Err(fail("이 파일은 작업이나 프로젝트에서 사용 중이에요.", 409));
        }
        sqlx::query("DELETE FROM ai_store.uploads WHERE id=$1 AND owner=$2")
            .bind(upload)
            .bind(owner)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        self.remove_upload_files(upload)
    }
    pub async fn enqueue(
        &self,
        owner: &str,
        kind: &str,
        data: Value,
        title: &str,
    ) -> Result<Value> {
        let data = self.validate(owner, kind, data).await?;
        let mut tx = self.owner_lock(owner).await?;
        let job = self.enqueue_tx(&mut tx, owner, kind, &data, title).await?;
        tx.commit().await?;
        self.job(owner, &job, true).await
    }
    pub(crate) async fn enqueue_tx(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        owner: &str,
        kind: &str,
        data: &Value,
        title: &str,
    ) -> Result<String> {
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended('cbgpu:ai:queue',0))")
            .execute(&mut **tx)
            .await?;
        let global: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM ai_store.jobs WHERE state IN('queued','running','paused')",
        )
        .fetch_one(&mut **tx)
        .await?;
        let active:i64=sqlx::query_scalar("SELECT count(*) FROM ai_store.jobs WHERE owner=$1 AND state IN('queued','running','paused')").bind(owner).fetch_one(&mut **tx).await?;
        if global >= 200 || active >= 5 {
            return Err(fail(
                "Finish or cancel an existing task first (maximum 5 active tasks).",
                429,
            ));
        }
        if let Some(upload) = data["upload_id"].as_str() {
            let association: Option<Option<String>> = sqlx::query_scalar(
                "SELECT job_id FROM ai_store.uploads WHERE id=$1 AND owner=$2 FOR UPDATE",
            )
            .bind(upload)
            .bind(owner)
            .fetch_optional(&mut **tx)
            .await?;
            let associated = association.ok_or_else(|| fail("Upload not found", 404))?;
            let project: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM ai_store.ai_project_files WHERE upload=$1)",
            )
            .bind(upload)
            .fetch_one(&mut **tx)
            .await?;
            if associated.is_some() || project {
                return Err(fail(
                    "This upload already belongs to a task or project; upload another copy",
                    409,
                ));
            }
        }
        let job = id();
        let folder = self.root.join("jobs").join(&job);
        private_dir(&folder)?;
        let created:Result<()>=async{if kind=="python"&&self.data_root.is_some(){let workspace=self.bulk_directory("workspaces")?.join(&job);private_dir(&workspace)?;#[cfg(unix)]std::os::unix::fs::symlink(workspace,folder.join("workspace"))?;}atomic_json(&folder.join("input.json"),data)?;let t=now();sqlx::query("INSERT INTO ai_store.jobs(id,owner,kind,title,state,created,updated,input)VALUES($1,$2,$3,$4,'queued',$5,$5,$6)").bind(&job).bind(owner).bind(kind).bind(if title.is_empty(){kind.to_owned()}else{title.chars().take(160).collect()}).bind(t).bind(data).execute(&mut **tx).await?;if let Some(upload)=data["upload_id"].as_str(){sqlx::query("UPDATE ai_store.uploads SET job_id=$1 WHERE id=$2 AND owner=$3").bind(&job).bind(upload).bind(owner).execute(&mut **tx).await?;}Ok(())}.await;
        if let Err(e) = created {
            let _ = self.remove_job_files(&job);
            return Err(e);
        }
        Ok(job)
    }
    pub async fn job(&self, owner: &str, job: &str, detail: bool) -> Result<Value> {
        if !valid_id(job) {
            return Err(fail("Task not found", 404));
        }
        let mut value: Value = sqlx::query_scalar(
            "SELECT to_jsonb(j)-'input' FROM ai_store.jobs j WHERE id=$1 AND owner=$2",
        )
        .bind(job)
        .bind(owner)
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| fail("Task not found", 404))?;
        if detail {
            let folder = self.root.join("jobs").join(job);
            if value["state"] == "succeeded" && folder.join("result.json").exists() {
                match read_json(&folder.join("result.json")) {
                    Ok(v) => value["result"] = v,
                    Err(_) => value["error"] = json!("Result is temporarily unavailable"),
                }
            }
            if let Ok(v) = read_json(&folder.join("partial.json")) {
                value["partial"] = json!(
                    v["text"]
                        .as_str()
                        .unwrap_or("")
                        .chars()
                        .take(100000)
                        .collect::<String>()
                );
            }
            if let Ok(mut f) = open_regular(&folder.join("log.txt")) {
                let len = f.metadata()?.len();
                f.seek(SeekFrom::Start(len.saturating_sub(24000)))?;
                let mut bytes = Vec::new();
                f.take(24000).read_to_end(&mut bytes)?;
                value["logs"] = json!(String::from_utf8_lossy(&bytes));
            }
            value["artifacts"] = json!(self.artifacts(job)?);
        }
        Ok(value)
    }
    pub async fn jobs(&self, owner: &str) -> Result<Vec<Value>> {
        Ok(sqlx::query_scalar("SELECT to_jsonb(j)-'input' FROM ai_store.jobs j WHERE owner=$1 AND state<>'deleted' ORDER BY created DESC LIMIT 100").bind(owner).fetch_all(&self.pool).await?)
    }
    pub async fn cancel_job(&self, owner: &str, job: &str) -> Result<()> {
        self.job(owner, job, false).await?;
        sqlx::query("UPDATE ai_store.jobs SET state='cancelled',progress='Cancelled by owner',updated=$1 WHERE id=$2 AND owner=$3 AND state IN('queued','running','paused')").bind(now()).bind(job).bind(owner).execute(&self.pool).await?;
        Ok(())
    }
    pub async fn delete_job(&self, owner: &str, job: &str) -> Result<()> {
        self.job(owner, job, false).await?;
        sqlx::query("UPDATE ai_store.jobs SET state='deleted',updated=$1 WHERE id=$2 AND owner=$3")
            .bind(now())
            .bind(job)
            .bind(owner)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
    pub async fn retry_job(&self, owner: &str, job: &str) -> Result<Value> {
        let mut tx = self.owner_lock(owner).await?;
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended('cbgpu:ai:queue',0))")
            .execute(&mut *tx)
            .await?;
        let count:i64=sqlx::query_scalar("SELECT count(*) FROM ai_store.jobs WHERE state IN('queued','running','paused') AND owner=$1").bind(owner).fetch_one(&mut *tx).await?;
        let global: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM ai_store.jobs WHERE state IN('queued','running','paused')",
        )
        .fetch_one(&mut *tx)
        .await?;
        if count >= 5 || global >= 200 {
            return Err(fail("Queue is full", 429));
        }
        let changed=sqlx::query("UPDATE ai_store.jobs SET state='queued',runtime=0,finished=NULL,error=NULL,progress='',updated=$1 WHERE id=$2 AND owner=$3 AND state IN('failed','cancelled')").bind(now()).bind(job).bind(owner).execute(&mut *tx).await?.rows_affected();
        if changed == 0 {
            return Err(fail("Task cannot be retried", 409));
        }
        tx.commit().await?;
        self.job(owner, job, true).await
    }
    pub fn workspace_root(&self, job: &str) -> Result<PathBuf> {
        if !valid_id(job) {
            return Err(fail("File not found", 404));
        }
        let workspace = self.root.join("jobs").join(job).join("workspace");
        if workspace.is_symlink() {
            let expected = self
                .data_root
                .as_ref()
                .ok_or_else(|| fail("File not found", 404))?
                .join("workspaces")
                .join(job);
            if workspace.canonicalize().ok() != Some(expected.clone()) {
                return Err(fail("File not found", 404));
            }
            Ok(expected)
        } else {
            Ok(workspace)
        }
    }
    pub fn open_artifact(&self, job: &str, relative: &str) -> Result<File> {
        let relative = safe_relative(relative)?;
        let root = self.workspace_root(job)?;
        open_beneath(&root, &PathBuf::from("output").join(relative))
            .map_err(|_| fail("File not found", 404))
    }
    pub async fn artifact(&self, owner: &str, job: &str, relative: &str) -> Result<File> {
        self.job(owner, job, false).await?;
        self.open_artifact(job, relative)
    }
    pub fn artifacts(&self, job: &str) -> Result<Vec<Value>> {
        let root = self.workspace_root(job)?;
        let output = root.join("output");
        if output.is_symlink() || !output.is_dir() {
            return Ok(vec![]);
        }
        let mut stack = vec![output.clone()];
        let mut result = vec![];
        let mut seen = 0;
        while let Some(dir) = stack.pop() {
            for entry in fs::read_dir(dir)? {
                let entry = entry?;
                seen += 1;
                if seen > 5000 || result.len() >= 1000 {
                    return Ok(result);
                }
                let ty = entry.file_type()?;
                if ty.is_dir() {
                    stack.push(entry.path());
                } else if ty.is_file() {
                    let p = entry.path();
                    let Some(relative) = p.strip_prefix(&output).ok().and_then(Path::to_str) else {
                        continue;
                    };
                    if let Ok(f) = self.open_artifact(job, relative) {
                        result.push(json!({"path":relative,"size":f.metadata()?.len()}));
                    }
                }
            }
        }
        Ok(result)
    }
    pub fn remove_job_files(&self, job: &str) -> Result<()> {
        if !valid_id(job) {
            return Err(fail("Invalid job ID", 400));
        }
        let folder = self.root.join("jobs").join(job);
        let workspace = folder.join("workspace");
        if workspace.is_symlink()
            && let Some(root) = &self.data_root
        {
            let expected = root.join("workspaces").join(job);
            if workspace.canonicalize().ok() == Some(expected.clone()) {
                fs::remove_dir_all(expected)?;
            }
        }
        if folder.exists() {
            fs::remove_dir_all(folder)?;
        }
        Ok(())
    }
    pub async fn keys(&self, owner: &str) -> Result<Vec<Value>> {
        Ok(sqlx::query_scalar("SELECT jsonb_build_object('id',id,'name',name,'prefix',prefix,'created',created,'last_used',last_used) FROM ai_store.api_keys WHERE owner=$1 AND revoked=0 ORDER BY created DESC").bind(owner).fetch_all(&self.pool).await?)
    }
    pub async fn create_key(&self, owner: &str, name: &str) -> Result<Value> {
        let mut bytes = [0u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut bytes);
        let token = format!(
            "cbgpu_ai_{}",
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
        );
        let key = id();
        let prefix = token.chars().take(17).collect::<String>();
        let name = if name.is_empty() {
            "API key".into()
        } else {
            name.chars().take(80).collect::<String>()
        };
        sqlx::query("INSERT INTO ai_store.api_keys(id,owner,name,prefix,hash,created)VALUES($1,$2,$3,$4,$5,$6)").bind(&key).bind(owner).bind(name).bind(&prefix).bind(hash_key(&token)).bind(now()).execute(&self.pool).await?;
        Ok(json!({"id":key,"key":token,"prefix":prefix}))
    }
    pub async fn key_owner(&self, token: &str) -> Result<String> {
        let owner: Option<String> = sqlx::query_scalar(
            "UPDATE ai_store.api_keys SET last_used=$1 WHERE hash=$2 AND revoked=0 RETURNING owner",
        )
        .bind(now())
        .bind(hash_key(token))
        .fetch_optional(&self.pool)
        .await?;
        owner.ok_or_else(|| fail("Invalid API key", 401))
    }
    pub async fn revoke_key(&self, owner: &str, key: &str) -> Result<()> {
        sqlx::query("UPDATE ai_store.api_keys SET revoked=1 WHERE id=$1 AND owner=$2")
            .bind(key)
            .bind(owner)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
    pub async fn download_ticket(
        &self,
        owner: &str,
        job: &str,
        kind: &str,
        path: &str,
    ) -> Result<Value> {
        let value = self.job(owner, job, true).await?;
        if value["state"] == "deleted" {
            return Err(fail("Task not found", 404));
        }
        if kind == "artifact" {
            self.open_artifact(job, path)?;
        } else {
            result_text(&value, kind)?;
        }
        let mut tx = self.owner_lock(owner).await?;
        sqlx::query("DELETE FROM ai_store.downloads WHERE expires<$1")
            .bind(now())
            .execute(&mut *tx)
            .await?;
        let count: i64 =
            sqlx::query_scalar("SELECT count(*) FROM ai_store.downloads WHERE owner=$1")
                .bind(owner)
                .fetch_one(&mut *tx)
                .await?;
        if count >= 100 {
            return Err(fail("Too many active downloads", 429));
        }
        let ticket = id();
        sqlx::query("INSERT INTO ai_store.downloads(id,owner,job_id,kind,path,expires)VALUES($1,$2,$3,$4,$5,$6)").bind(&ticket).bind(owner).bind(job).bind(kind).bind(path).bind(now()+600.).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(json!({"id":ticket}))
    }
    pub async fn download(&self, owner: &str, ticket: &str) -> Result<Download> {
        let row: Value = sqlx::query_scalar(
            "SELECT to_jsonb(d) FROM ai_store.downloads d WHERE id=$1 AND owner=$2 AND expires>$3",
        )
        .bind(ticket)
        .bind(owner)
        .bind(now())
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| fail("Download not found or expired", 404))?;
        let job = row["job_id"]
            .as_str()
            .ok_or_else(|| fail("Invalid download", 404))?;
        let value = self.job(owner, job, true).await?;
        if value["state"] == "deleted" {
            return Err(fail("Task not found", 404));
        }
        if row["kind"] == "artifact" {
            Ok(Download::Artifact(
                self.open_artifact(job, row["path"].as_str().unwrap_or(""))?,
            ))
        } else {
            Ok(Download::Bytes(
                result_text(&value, row["kind"].as_str().unwrap_or(""))?
                    .as_bytes()
                    .to_vec(),
            ))
        }
    }
    pub async fn cached_vector(&self, hash: &str) -> Result<Option<Value>> {
        Ok(
            sqlx::query_scalar("SELECT vector FROM ai_store.vectors WHERE hash=$1")
                .bind(hash)
                .fetch_optional(&self.pool)
                .await?,
        )
    }
    pub async fn cache_vector(&self, hash: &str, vector: &Value) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended('cbgpu:ai:vectors',0))")
            .execute(&mut *tx)
            .await?;
        sqlx::query("INSERT INTO ai_store.vectors(hash,vector,updated)VALUES($1,$2,$3)ON CONFLICT(hash)DO UPDATE SET vector=excluded.vector,updated=excluded.updated").bind(hash).bind(vector).bind(now()).execute(&mut *tx).await?;
        sqlx::query("DELETE FROM ai_store.vectors WHERE hash IN(SELECT hash FROM ai_store.vectors ORDER BY updated DESC,hash OFFSET 5000)").execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(())
    }
}
pub enum Download {
    Artifact(File),
    Bytes(Vec<u8>),
}
fn result_text<'a>(job: &'a Value, kind: &str) -> Result<&'a str> {
    let r = &job["result"];
    let value = match kind {
        "srt" => r["srt"].as_str(),
        "text" => r["text"]
            .as_str()
            .or_else(|| r["choices"][0]["message"]["content"].as_str()),
        _ => return Err(fail("Invalid download type", 400)),
    };
    value.ok_or_else(|| fail("Result not found", 404))
}
pub fn hash_key(value: &str) -> String {
    Sha256::digest(value.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
fn read_json(path: &Path) -> Result<Value> {
    Ok(serde_json::from_reader(open_regular(path)?)?)
}
fn remove_optional(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}
#[cfg(unix)]
fn available(path: &Path) -> Result<u64> {
    use std::os::unix::ffi::OsStrExt;
    let path = std::ffi::CString::new(path.as_os_str().as_bytes())
        .map_err(|_| fail("Invalid path", 400))?;
    let mut s = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    let result = unsafe { libc::statvfs(path.as_ptr(), s.as_mut_ptr()) };
    if result != 0 {
        return Err(fail("Storage status unavailable", 503));
    }
    let s = unsafe { s.assume_init() };
    Ok(s.f_bavail.saturating_mul(s.f_frsize))
}
#[cfg(not(unix))]
fn available(_: &Path) -> Result<u64> {
    Err(fail("Disk safety check unavailable on this host", 503))
}
#[cfg(unix)]
fn open_beneath(root: &Path, relative: &Path) -> std::io::Result<File> {
    use std::os::{
        fd::{AsRawFd, FromRawFd},
        unix::fs::OpenOptionsExt,
    };
    let mut directory = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
        .open(root)?;
    let parts = relative
        .components()
        .filter_map(|p| {
            if let Component::Normal(p) = p {
                Some(p)
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    for (index, part) in parts.iter().enumerate() {
        use std::os::unix::ffi::OsStrExt;
        let name = std::ffi::CString::new(part.as_bytes())?;
        let last = index + 1 == parts.len();
        let fd = unsafe {
            libc::openat(
                directory.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY
                    | libc::O_NOFOLLOW
                    | libc::O_CLOEXEC
                    | if last {
                        libc::O_NONBLOCK
                    } else {
                        libc::O_DIRECTORY
                    },
            )
        };
        if fd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        let file = unsafe { File::from_raw_fd(fd) };
        if last {
            return if file.metadata()?.is_file() {
                Ok(file)
            } else {
                Err(std::io::ErrorKind::NotFound.into())
            };
        }
        directory = file;
    }
    Err(std::io::ErrorKind::NotFound.into())
}
#[cfg(not(unix))]
fn open_beneath(_: &Path, _: &Path) -> std::io::Result<File> {
    Err(std::io::ErrorKind::Unsupported.into())
}

fn open_regular(path: &Path) -> std::io::Result<File> {
    let mut o = OpenOptions::new();
    o.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let f = o.open(path)?;
    if !f.metadata()?.is_file() {
        return Err(std::io::ErrorKind::NotFound.into());
    }
    Ok(f)
}
