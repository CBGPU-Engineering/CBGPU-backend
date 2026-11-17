//! Native worker supervision. Customer stdout is not authority for filesystem/container ownership.
use crate::{Result, api::Api, fail, now, queue::Work, store::Store, valid_id};
use serde_json::{Value, json};
use sqlx::Row;
use std::{
    path::{Path, PathBuf},
    process::Stdio,
    sync::Arc,
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt},
    sync::mpsc,
};
#[async_trait::async_trait]
pub trait Executor: Send + Sync {
    async fn execute(&self, api: Arc<Api>, work: &Work) -> Result<Option<Value>>;
    async fn maintain(&self, _api: &Api) -> Result<()> {
        Ok(())
    }
    async fn remote_head(
        &self,
        _project: &crate::store::Project,
        _cipher: &crate::crypto::Cipher,
    ) -> Result<Option<String>> {
        Ok(None)
    }
}
pub struct Controller {
    pub api: Arc<Api>,
    pub executor: Arc<dyn Executor>,
}
impl Controller {
    pub async fn step(&self, kinds: &[String]) -> Result<bool> {
        let Some(work) = self.api.store.claim(kinds).await? else {
            return Ok(false);
        };
        let outcome = self.executor.execute(self.api.clone(), &work).await;
        match outcome {
            Ok(result) => {
                self.api
                    .store
                    .finish_work(&work, result.as_ref(), None)
                    .await?
            }
            Err(error) => {
                self.api
                    .store
                    .finish_work(&work, None, Some(&error.message))
                    .await?
            }
        }
        if work.kind == "compose_build" {
            let status: String =
                sqlx::query_scalar("SELECT state FROM deploy_store.work_items WHERE id=$1")
                    .bind(&work.id)
                    .fetch_one(&self.api.store.pool)
                    .await?;
            let time = now();
            let deployed = if status == "succeeded" {
                Some(time.as_str())
            } else {
                None
            };
            if self
                .api
                .authority
                .mark_deployed(&work.project, deployed)
                .await
                .is_err()
            {
                sqlx::query("UPDATE deploy_store.compose_states SET status='error',error='Server state synchronization failed; containers and volumes retained',updated_at=$1 WHERE project_id=$2").bind(now()).bind(&work.project).execute(&self.api.store.pool).await?;
            }
        }
        Ok(true)
    }
    pub async fn auto_poll(&self) -> Result<usize> {
        let rows=sqlx::query("SELECT p.id,p.owner,d.result FROM deploy_store.projects p JOIN deploy_store.deployments d ON d.id=p.production AND d.project_id=p.id WHERE p.deleted=0 AND d.status='ready' AND coalesce((p.config->>'auto_deploy')::boolean,false) AND NOT EXISTS(SELECT 1 FROM deploy_store.deployments active WHERE active.project_id=p.id AND active.status IN('queued','building'))").fetch_all(&self.api.store.pool).await?;
        let mut scheduled = 0;
        for row in rows {
            let owner: String = row.get("owner");
            if !self
                .api
                .authority
                .can_execute(&owner)
                .await
                .unwrap_or(false)
            {
                continue;
            }
            let p = match self
                .api
                .store
                .find(&row.get::<String, _>("id"), &owner)
                .await
            {
                Ok(p) => p,
                Err(_) => continue,
            };
            let head = match self.executor.remote_head(&p, &self.api.store.cipher).await {
                Ok(Some(head)) => head,
                _ => continue,
            };
            if row.get::<Value, _>("result")["commit"] != head
                && self.api.store.schedule(&p, &owner).await.is_ok()
            {
                scheduled += 1;
            }
        }
        Ok(scheduled)
    }
    pub async fn reconcile(&self) -> Result<()> {
        self.executor.maintain(&self.api).await?;
        for p in self.api.authority.server_projects().await? {
            if p["env_vars"]["CBGPU_DEPLOY_ENGINE"] != "compose" {
                continue;
            }
            let id = p["id"]
                .as_str()
                .filter(|v| valid_id(v))
                .ok_or_else(|| fail("Invalid server identity", 503))?;
            if p["status"] == "deleted" {
                let mut tx = self.api.store.pool.begin().await?;
                let exists:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM deploy_store.compose_states WHERE project_id=$1 AND status<>'stopped')AND NOT EXISTS(SELECT 1 FROM deploy_store.work_items WHERE project_id=$1 AND kind='compose_stop'AND state IN('queued','running'))").bind(id).fetch_one(&mut*tx).await?;
                if exists {
                    sqlx::query("UPDATE deploy_store.work_items SET cancel_requested=true WHERE project_id=$1 AND kind='compose_build'AND state IN('queued','running')").bind(id).execute(&mut*tx).await?;
                    Store::enqueue(
                        &mut tx,
                        "compose_stop",
                        "server",
                        id,
                        p["user_id"].as_str().unwrap_or(""),
                        None,
                        json!({"deleted":true}),
                    )
                    .await?;
                }
                tx.commit().await?;
            } else if p["status"] == "approved"
                && p["deploy_requested_at"].is_string()
                && self
                    .api
                    .authority
                    .can_execute(p["user_id"].as_str().unwrap_or(""))
                    .await?
            {
                self.api.store.enqueue_compose(&p).await?;
            }
        }
        Ok(())
    }
}
pub struct Native {
    pub front: PathBuf,
    pub compose: PathBuf,
    pub domains: PathBuf,
    pub backend_root: PathBuf,
    pub compose_base: PathBuf,
    pub home: PathBuf,
    pub tool_dir: Option<PathBuf>,
    pub docker_socket: PathBuf,
    pub last_measure: tokio::sync::Mutex<Option<std::time::Instant>>,
    pub log_proxies: tokio::sync::Mutex<std::collections::HashMap<String, crate::log_proxy::Proxy>>,
}
fn native(path: &Path) -> Result<()> {
    use std::io::Read;
    if !path.is_absolute() {
        return Err(fail(
            "Worker paths must be trusted absolute ELF executables",
            503,
        ));
    }
    let mut magic = [0; 4];
    std::fs::File::open(path)?.read_exact(&mut magic)?;
    if magic != *b"\x7fELF" {
        return Err(fail("Only native workers may be attached", 503));
    }
    Ok(())
}
impl Native {
    async fn ensure_log_proxy(&self, id: &str) -> Result<()> {
        if !valid_id(id) {
            return Err(fail("Invalid Compose identity", 503));
        }
        let mut proxies = self.log_proxies.lock().await;
        if proxies.get(id).is_some_and(|p| !p.healthy()) {
            proxies.remove(id);
        }
        if !proxies.contains_key(id) {
            let socket = self
                .compose_base
                .parent()
                .ok_or_else(|| fail("Missing Compose parent", 503))?
                .join("compose-sockets")
                .join(format!("{id}.sock"));
            let proxy = crate::log_proxy::start(
                &socket,
                &self.docker_socket,
                &format!("cbgpu-compose-{id}"),
            )
            .await?;
            proxies.insert(id.into(), proxy);
        }
        Ok(())
    }
    pub fn validate(&self) -> Result<()> {
        for path in [&self.front, &self.compose, &self.domains] {
            native(path)?
        }
        if !self.backend_root.is_absolute()
            || !self.compose_base.is_absolute()
            || !self.docker_socket.is_absolute()
        {
            return Err(fail("Use explicit private runtime paths", 503));
        }
        Ok(())
    }
    fn tool(&self, name: &str) -> PathBuf {
        self.tool_dir
            .as_ref()
            .map(|p| p.join(name))
            .unwrap_or_else(|| name.into())
    }
    async fn docker(&self, args: &[String]) -> Result<String> {
        let output = tokio::time::timeout(
            Duration::from_secs(60),
            tokio::process::Command::new(self.tool("docker"))
                .args(args)
                .kill_on_drop(true)
                .output(),
        )
        .await
        .map_err(|_| fail("Docker operation timed out", 503))??;
        if !output.status.success() {
            return Err(fail("Owned deployment operation failed", 503));
        }
        if output.stdout.len() > 4 * 1024 * 1024 {
            return Err(fail("Docker response exceeds limit", 503));
        }
        String::from_utf8(output.stdout).map_err(|_| fail("Invalid Docker response", 503))
    }
    async fn remove_front(&self, id: &str, runtime: bool) -> Result<()> {
        if !valid_id(id) {
            return Err(fail("Invalid deployment ownership", 503));
        }
        for (kind, label) in if runtime {
            vec![
                ("build", "cbgpu.frontend.build"),
                ("run", "cbgpu.frontend.runtime"),
            ]
        } else {
            vec![("build", "cbgpu.frontend.build")]
        } {
            let name = format!("cbfront-{kind}-{id}");
            let found = self
                .docker(&[
                    "inspect".into(),
                    "--format".into(),
                    "{{json .Config.Labels}}".into(),
                    name.clone(),
                ])
                .await;
            if let Ok(raw) = found {
                let value: Value = serde_json::from_str(&raw)?;
                if value[label] != "true" {
                    return Err(fail("Refusing to remove an unowned container", 403));
                }
                self.docker(&["rm".into(), "-f".into(), name]).await?;
            }
        }
        Ok(())
    }
    async fn front_cleanup(&self, api: &Api, w: &Work) -> Result<()> {
        let rows =
            sqlx::query("SELECT id,result FROM deploy_store.deployments WHERE project_id=$1")
                .bind(&w.project)
                .fetch_all(&api.store.pool)
                .await?;
        for row in rows {
            let id: String = row.get("id");
            if let Some(selected) = w.payload["deployment_id"].as_str()
                && selected != id
            {
                continue;
            }
            let mut tx = api.store.pool.begin().await?;
            let p = sqlx::query(
                "SELECT production,deleted FROM deploy_store.projects WHERE id=$1 FOR UPDATE",
            )
            .bind(&w.project)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or_else(|| fail("Project missing", 404))?;
            if p.get::<i32, _>("deleted") == 0
                && p.get::<Option<String>, _>("production").as_deref() == Some(&id)
            {
                tx.rollback().await?;
                continue;
            }
            sqlx::query(
                "UPDATE deploy_store.deployments SET status='expired',result=NULL WHERE id=$1",
            )
            .bind(&id)
            .execute(&mut *tx)
            .await?;
            tx.commit().await?;
            self.remove_front(&id, true).await?;
            let directory = api.store.data.join(&id);
            if directory.exists() {
                if directory.canonicalize()? != api.store.data.canonicalize()?.join(&id) {
                    return Err(fail("Deployment directory ownership mismatch", 403));
                }
                std::fs::remove_dir_all(directory)?;
            }
        }
        if w.payload["deleted"] == true {
            let mut tx = api.store.pool.begin().await?;
            for row in sqlx::query("SELECT hostname,provider_id FROM deploy_store.custom_domains WHERE resource_type='frontend'AND project_id=$1").bind(&w.project).fetch_all(&mut*tx).await?{if let Some(provider)=row.get::<Option<String>,_>("provider_id"){Store::enqueue(&mut tx,"domain_remove","frontend",&w.project,&w.owner,None,json!({"hostname":row.get::<String,_>("hostname"),"provider_id":provider})).await?;}}
            sqlx::query("DELETE FROM deploy_store.custom_domains WHERE resource_type='frontend'AND project_id=$1").bind(&w.project).execute(&mut*tx).await?;
            tx.commit().await?;
        }
        Ok(())
    }
    async fn stop_compose(&self, api: &Api, w: &Work) -> Result<()> {
        let state = sqlx::query(
            "SELECT result,attempt_result FROM deploy_store.compose_states WHERE project_id=$1",
        )
        .bind(&w.project)
        .fetch_optional(&api.store.pool)
        .await?;
        if let Some(state) = state {
            let result: Option<Value> = state
                .get::<Option<Value>, _>("attempt_result")
                .or_else(|| state.get("result"));
            if let Some(result) = result {
                let file = validate_attempt(&result, &self.compose_base, &w.project)?;
                self.docker(&[
                    "compose".into(),
                    "-p".into(),
                    format!("cbgpu-compose-{}", w.project),
                    "-f".into(),
                    file.to_string_lossy().into_owned(),
                    "down".into(),
                    "--remove-orphans".into(),
                    "--timeout".into(),
                    "20".into(),
                ])
                .await?;
            }
        }
        sqlx::query("UPDATE deploy_store.compose_states SET status='stopped',updated_at=$1 WHERE project_id=$2").bind(now()).bind(&w.project).execute(&api.store.pool).await?;
        self.log_proxies.lock().await.remove(&w.project);
        let _ = api.authority.mark_deployed(&w.project, None).await;
        Ok(())
    }
    async fn domain_operation(&self, api: &Api, w: &Work) -> Result<()> {
        let hostname = w.payload["hostname"]
            .as_str()
            .ok_or_else(|| fail("Domain job malformed", 503))?;
        crate::config::hostname(hostname)?;
        let mut args = vec![
            "--root".to_owned(),
            self.backend_root.to_string_lossy().into_owned(),
            "custom-tls".into(),
        ];
        if w.kind == "domain_remove" {
            let provider = w.payload["provider_id"]
                .as_str()
                .ok_or_else(|| fail("Domain provider missing", 503))?;
            let used:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM deploy_store.custom_domains WHERE hostname=$1 AND dns_verified=1 AND(provider_id=$2 OR provider_id IS NULL))").bind(hostname).bind(provider).fetch_one(&api.store.pool).await?;
            if used {
                return Ok(());
            }
            args.extend(["remove".into(), provider.into()]);
        } else {
            let id = w.payload["domain_id"]
                .as_str()
                .ok_or_else(|| fail("Domain job missing", 503))?;
            let row=sqlx::query("SELECT verification_token FROM deploy_store.custom_domains WHERE id=$1 AND hostname=$2 AND dns_verified=1").bind(id).bind(hostname).fetch_optional(&api.store.pool).await?;
            let Some(row) = row else { return Ok(()) };
            let token: String = row.get("verification_token");
            if !api
                .authority
                .txt_ownership(hostname, &format!("cbgpu-domain-verification={token}"))
                .await?
            {
                sqlx::query("UPDATE deploy_store.custom_domains SET dns_verified=0,tls_status='pending_dns',last_check=$1 WHERE id=$2").bind(now()).bind(id).execute(&api.store.pool).await?;
                return Ok(());
            }
            args.extend([
                "ensure".into(),
                hostname.into(),
                w.payload["provider_id"].as_str().unwrap_or("").into(),
            ]);
        }
        let output = tokio::time::timeout(
            Duration::from_secs(40),
            tokio::process::Command::new(&self.domains)
                .args(args)
                .env_clear()
                .env("PATH", "/usr/bin:/bin")
                .env("HOME", &self.home)
                .kill_on_drop(true)
                .output(),
        )
        .await
        .map_err(|_| fail("Domain provider timed out", 503))??;
        if w.kind == "domain_ensure" {
            let id = w.payload["domain_id"].as_str().unwrap();
            if !output.status.success() || output.stdout.len() > 65536 {
                sqlx::query("UPDATE deploy_store.custom_domains SET tls_status='provider_setup_required'WHERE id=$1").bind(id).execute(&api.store.pool).await?;
                return Err(fail("Domain provider setup requires review", 503));
            }
            let result: Value = serde_json::from_slice(&output.stdout)?;
            sqlx::query("UPDATE deploy_store.custom_domains SET tls_status=$1,provider_id=$2,provider_records=$3 WHERE id=$4 AND dns_verified=1 AND hostname=$5").bind(result["status"].as_str().unwrap_or("pending_tls")).bind(result["id"].as_str()).bind(result["records"].as_array().map(|v|json!(v)).unwrap_or(json!([]))).bind(id).bind(hostname).execute(&api.store.pool).await?;
        }
        Ok(())
    }
}
#[derive(Clone)]
struct Redactor(Vec<String>);
impl Redactor {
    fn new(value: &Value) -> Self {
        let mut secrets = vec![];
        fn collect(value: &Value, out: &mut Vec<String>) {
            match value {
                Value::String(s) if !s.is_empty() => {
                    out.push(s.clone());
                    out.extend(s.lines().filter(|s| !s.is_empty()).map(str::to_owned));
                }
                Value::Object(v) => {
                    for value in v.values() {
                        collect(value, out)
                    }
                }
                _ => {}
            }
        }
        collect(value, &mut secrets);
        secrets.sort_by_key(|s| std::cmp::Reverse(s.len()));
        secrets.dedup();
        Self(secrets)
    }
    fn text(&self, value: &str) -> String {
        let mut value = value.to_owned();
        for secret in &self.0 {
            value = value.replace(secret, "[REDACTED]")
        }
        value
    }
    fn json(&self, value: &Value) -> Value {
        match value {
            Value::String(v) => json!(self.text(v)),
            Value::Array(v) => Value::Array(v.iter().map(|v| self.json(v)).collect()),
            Value::Object(v) => {
                Value::Object(v.iter().map(|(k, v)| (k.clone(), self.json(v))).collect())
            }
            _ => value.clone(),
        }
    }
}
async fn lines(
    reader: impl AsyncRead + Unpin,
    tx: mpsc::Sender<std::result::Result<(bool, String), ()>>,
    stdout: bool,
) {
    let mut reader = reader;
    let mut buffer = [0; 4096];
    let mut pending = vec![];
    loop {
        match reader.read(&mut buffer).await {
            Ok(0) => {
                if !pending.is_empty() {
                    let _ = tx
                        .send(Ok((stdout, String::from_utf8_lossy(&pending).into_owned())))
                        .await;
                }
                break;
            }
            Ok(n) => {
                for byte in &buffer[..n] {
                    if *byte == b'\n' {
                        if tx
                            .send(Ok((stdout, String::from_utf8_lossy(&pending).into_owned())))
                            .await
                            .is_err()
                        {
                            return;
                        }
                        pending.clear();
                    } else {
                        pending.push(*byte);
                        if pending.len() > 32768 {
                            let _ = tx.send(Err(())).await;
                            return;
                        }
                    }
                }
            }
            Err(_) => {
                let _ = tx.send(Err(())).await;
                break;
            }
        }
    }
}
fn compose_actor<'a>(
    payload: &'a Value,
    current: &'a Value,
    owner: &'a str,
    required: bool,
) -> Option<&'a str> {
    if (required || payload["requested_by"].as_str().is_some())
        && (payload["requested_at"].as_str().is_none()
            || payload["requested_at"]
                .as_str()
                .is_some_and(|v| current["deploy_requested_at"].as_str() != Some(v)))
    {
        return None;
    }
    match (
        payload["requested_by"].as_str(),
        current["deploy_requested_by"].as_str(),
    ) {
        (Some(actor), Some(fresh)) if actor == fresh && crate::valid_id(actor) => Some(actor),
        (None, None) if !required => Some(owner),
        _ => None,
    }
}
async fn admitted(api: &Api, w: &Work) -> Result<bool> {
    if !api.store.continuing(w).await? {
        return Ok(false);
    }
    if w.kind == "front_build" {
        let actor = w.payload["requested_by"].as_str().unwrap_or(&w.owner);
        let p = api.store.find(&w.project, actor).await?;
        if p.owner != w.owner || p.role == "viewer" {
            return Ok(false);
        }
        if !api.authority.can_execute(&w.owner).await?
            || actor != w.owner && !api.authority.can_execute(actor).await?
        {
            return Ok(false);
        }
    }
    if w.kind == "compose_build" {
        let p = api.authority.server_project(&w.project).await?;
        let actor = match compose_actor(
            &w.payload,
            &p,
            &w.owner,
            api.authority.requires_request_actor(),
        ) {
            Some(actor) => actor,
            None => return Ok(false),
        };
        let role = api
            .store
            .role("server", &w.project, &w.owner, actor)
            .await?;
        if p["status"] != "approved"
            || p["user_id"] != w.owner
            || !matches!(role.as_str(), "owner" | "editor")
            || !api.authority.can_execute(&w.owner).await?
            || actor != w.owner && !api.authority.can_execute(actor).await?
        {
            return Ok(false);
        }
    }
    Ok(true)
}
#[async_trait::async_trait]
impl Executor for Native {
    async fn remote_head(
        &self,
        project: &crate::store::Project,
        cipher: &crate::crypto::Cipher,
    ) -> Result<Option<String>> {
        use base64::Engine;
        let secrets = cipher.unseal(&project.secrets)?;
        let (config, _) =
            crate::config::validate(&project.config, &serde_json::json!({}), &secrets)?;
        let repository = config["repository"]
            .as_str()
            .ok_or_else(|| fail("Missing repository", 400))?;
        let branch = config["branch"]
            .as_str()
            .ok_or_else(|| fail("Missing branch", 400))?;
        let mut command = tokio::process::Command::new(self.tool("git"));
        command
            .args([
                "ls-remote",
                "--heads",
                "--exit-code",
                "--",
                repository,
                &format!("refs/heads/{branch}"),
            ])
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", &self.home)
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_KEY_0", "protocol.file.allow")
            .env("GIT_CONFIG_VALUE_0", "never")
            .env("GIT_CONFIG_KEY_1", "http.followRedirects")
            .env("GIT_CONFIG_VALUE_1", "false")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        let token = secrets["git_token"].as_str().filter(|s| !s.is_empty());
        command.env("GIT_CONFIG_COUNT", if token.is_some() { "3" } else { "2" });
        if let Some(token) = token {
            command
                .env("GIT_CONFIG_KEY_2", "http.https://github.com/.extraheader")
                .env(
                    "GIT_CONFIG_VALUE_2",
                    format!(
                        "Authorization: Basic {}",
                        base64::engine::general_purpose::STANDARD
                            .encode(format!("x-access-token:{token}"))
                    ),
                );
        }
        let mut child = command.spawn()?;
        let mut stdout = child
            .stdout
            .take()
            .ok_or_else(|| fail("Git output unavailable", 503))?;
        let output = tokio::time::timeout(Duration::from_secs(20), async {
            let mut bytes = Vec::new();
            (&mut stdout).take(4097).read_to_end(&mut bytes).await?;
            let status = child.wait().await?;
            Ok::<_, std::io::Error>((status, bytes))
        })
        .await
        .map_err(|_| fail("Repository check timed out", 503))??;
        if !output.0.success() || output.1.len() > 4096 {
            return Ok(None);
        }
        let text =
            std::str::from_utf8(&output.1).map_err(|_| fail("Invalid repository response", 503))?;
        Ok(parse_remote_head(text, branch))
    }
    async fn maintain(&self, api: &Api) -> Result<()> {
        let ids:Vec<String>=sqlx::query_scalar("SELECT project_id FROM deploy_store.compose_states WHERE status<>'stopped' AND (result IS NOT NULL OR attempt_result IS NOT NULL)").fetch_all(&api.store.pool).await?;
        for id in ids {
            self.ensure_log_proxy(&id).await?;
        }
        let measure = {
            let mut last = self.last_measure.lock().await;
            let due = last.is_none_or(|t| t.elapsed() >= Duration::from_secs(300));
            if due {
                *last = Some(std::time::Instant::now());
            }
            due
        };
        let ready=sqlx::query("SELECT project_id,result,storage_gb FROM deploy_store.compose_states WHERE status='ready'AND result IS NOT NULL").fetch_all(&api.store.pool).await?;
        for row in ready {
            let id: String = row.get("project_id");
            let owner = api.authority.server_owner(&id).await?;
            let authorized = api.authority.can_execute(&owner).await?;
            let mut exceeded = false;
            if measure && authorized {
                let result: Value = row.get("result");
                validate_attempt(&result, &self.compose_base, &id)?;
                let mut child = tokio::process::Command::new(&self.compose)
                    .arg("--measure")
                    .env_clear()
                    .env("PATH", "/usr/local/bin:/usr/bin:/bin")
                    .env("HOME", &self.home)
                    .env("CBGPU_COMPOSE_BASE", &self.compose_base)
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::null())
                    .kill_on_drop(true)
                    .spawn()?;
                if let Some(mut input) = child.stdin.take() {
                    input.write_all(&serde_json::to_vec(&result)?).await?;
                }
                // Measurement output is bounded independently of the worker's exit status.
                let mut output = child
                    .stdout
                    .take()
                    .ok_or_else(|| fail("Measurement stream unavailable", 503))?;
                let measured = tokio::time::timeout(Duration::from_secs(30), async {
                    let mut bytes = Vec::new();
                    (&mut output).take(4097).read_to_end(&mut bytes).await?;
                    let status = child.wait().await?;
                    Ok::<_, std::io::Error>((status, bytes))
                })
                .await;
                if let Ok(Ok((status, bytes))) = measured
                    && status.success()
                    && bytes.len() <= 4096
                    && let Ok(value) = serde_json::from_slice::<Value>(&bytes)
                    && let Some(bytes) = value["bytes"].as_i64().filter(|v| *v >= 0)
                {
                    sqlx::query("UPDATE deploy_store.compose_states SET storage_used_bytes=$1 WHERE project_id=$2").bind(bytes).bind(&id).execute(&api.store.pool).await?;
                    exceeded = bytes > row.get::<i64, _>("storage_gb") * 1073741824;
                }
            }
            if !authorized || exceeded {
                let mut tx = api.store.pool.begin().await?;
                sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
                    .bind(format!("compose:{id}"))
                    .execute(&mut *tx)
                    .await?;
                let exists:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM deploy_store.work_items WHERE project_id=$1 AND kind='compose_stop'AND state IN('queued','running'))").bind(&id).fetch_one(&mut*tx).await?;
                if !exists {
                    Store::enqueue(&mut tx,"compose_stop","server",&id,&owner,None,json!({"reason":if exceeded{"Storage budget exceeded; volumes retained"}else{"Execution permission revoked; volumes retained"}})).await?;
                }
                tx.commit().await?;
            }
        }
        Ok(())
    }
    async fn execute(&self, api: Arc<Api>, w: &Work) -> Result<Option<Value>> {
        if !valid_id(&w.id) || !valid_id(&w.project) {
            return Err(fail("Malformed worker identity", 503));
        }
        match w.kind.as_str() {
            "front_cleanup" => {
                self.front_cleanup(&api, w).await?;
                return Ok(None);
            }
            "compose_stop" => {
                self.stop_compose(&api, w).await?;
                return Ok(None);
            }
            "domain_ensure" | "domain_remove" => {
                self.domain_operation(&api, w).await?;
                return Ok(None);
            }
            "front_build" | "compose_build" => {}
            _ => return Err(fail("Unsupported native work kind", 503)),
        }
        if !admitted(&api, w).await? {
            return Err(fail(
                "Execution permission revoked; project and previous production retained",
                403,
            ));
        }
        let (front, input, secrets) = if w.kind == "front_build" {
            let d = w
                .deployment
                .as_ref()
                .filter(|v| valid_id(v))
                .ok_or_else(|| fail("Invalid deployment identity", 503))?;
            let mut input = w.payload["config"].clone();
            let secrets = api.store.cipher.unseal(
                w.payload["secrets"]
                    .as_str()
                    .ok_or_else(|| fail("Build secret snapshot missing", 503))?,
            )?;
            for (k, v) in secrets
                .as_object()
                .ok_or_else(|| fail("Invalid build secrets", 503))?
            {
                input[k] = v.clone()
            }
            input["directory"] = json!(api.store.data.join(d));
            input["deployment_id"] = json!(d);
            (true, input, secrets)
        } else {
            self.ensure_log_proxy(&w.project).await?;
            // Reserve unconsumed budgets across projects on the actual Compose filesystem.
            let reserved:i64=sqlx::query_scalar("SELECT coalesce(sum(greatest(storage_gb*1073741824-coalesce(storage_used_bytes,0),0)),0)::bigint FROM deploy_store.compose_states WHERE project_id<>$1 AND status<>'stopped'").bind(&w.project).fetch_one(&api.store.pool).await?;
            let budget: i64 = sqlx::query_scalar(
                "SELECT storage_gb FROM deploy_store.compose_states WHERE project_id=$1",
            )
            .bind(&w.project)
            .fetch_one(&api.store.pool)
            .await?;
            use std::os::unix::ffi::OsStrExt;
            let path = std::ffi::CString::new(self.compose_base.as_os_str().as_bytes())
                .map_err(|_| fail("Invalid Compose storage", 503))?;
            let mut stats = std::mem::MaybeUninit::<libc::statvfs>::uninit();
            if unsafe { libc::statvfs(path.as_ptr(), stats.as_mut_ptr()) } != 0 {
                return Err(fail("Compose storage unavailable", 503));
            }
            let stats = unsafe { stats.assume_init() };
            let required = (budget.max(0) as u128 + 5) * 1073741824 + reserved.max(0) as u128;
            if (stats.f_bavail as u128) * (stats.f_frsize as u128) < required {
                return Err(fail(
                    "Insufficient Compose storage after reserved budgets",
                    503,
                ));
            }
            let mut input = api.store.cipher.unseal(
                w.payload["sealed_project"]
                    .as_str()
                    .ok_or_else(|| fail("Compose snapshot missing", 503))?,
            )?;
            let slug: Option<String> = sqlx::query_scalar(
                "SELECT slug FROM deploy_store.server_domains WHERE project_id=$1",
            )
            .bind(&w.project)
            .fetch_optional(&api.store.pool)
            .await?;
            input["public_url"] = json!(format!(
                "https://{}.server.cbgpu.com",
                slug.unwrap_or_else(|| format!("server-{}", &w.project[..8]))
            ));
            let mut secrets = input["env_vars"].clone();
            for name in [
                "CBGPU_ROOT_DIRECTORY",
                "CBGPU_GATEWAY_SERVICE",
                "CBGPU_STORAGE_GB",
                "CBGPU_COMPOSE_FILE",
                "CBGPU_DEPLOY_ENGINE",
                "CBGPU_RUNTIME_PRESET",
            ] {
                if let Some(values) = secrets.as_object_mut() {
                    values.remove(name);
                }
            }
            sqlx::query("UPDATE deploy_store.compose_states SET status='building',access_state='ALLOW',updated_at=$1 WHERE project_id=$2").bind(now()).bind(&w.project).execute(&api.store.pool).await?;
            (false, input, secrets)
        };
        let redactor = Redactor::new(&secrets);
        let bytes = serde_json::to_vec(&input)?;
        if bytes.len() > 131072 {
            return Err(fail("Worker input exceeds native ABI limit", 400));
        }
        let executable = if front { &self.front } else { &self.compose };
        native(executable)?;
        let mut command = tokio::process::Command::new(executable);
        command
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", &self.home)
            .env("CBGPU_COMPOSE_BASE", &self.compose_base)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        #[cfg(unix)]
        command.process_group(0);
        let mut child = command.spawn()?;
        let mut stdin = child.stdin.take().unwrap();
        stdin.write_all(&bytes).await?;
        stdin.shutdown().await?;
        drop(stdin);
        let (tx, mut rx) = mpsc::channel(16);
        let out = tokio::spawn(lines(child.stdout.take().unwrap(), tx.clone(), true));
        let err = tokio::spawn(lines(child.stderr.take().unwrap(), tx.clone(), false));
        drop(tx);
        let mut tick = tokio::time::interval(Duration::from_secs(1));
        let deadline =
            tokio::time::Instant::now() + Duration::from_secs(if front { 1800 } else { 3600 });
        let (mut status, mut open, mut result, mut diagnostic, mut attempt, mut logs) =
            (None, true, None, None, None, String::new());
        let mut failure = None;
        loop {
            tokio::select! {message=rx.recv(),if open=>{match message{Some(Ok((stdout,line)))=>{if stdout&&line.starts_with("CBGPU_RESULT "){result=serde_json::from_str(&line[13..]).ok();continue}
            if stdout&&line.starts_with("CBGPU_ATTEMPT "){attempt=serde_json::from_str::<Value>(&line[14..]).ok().filter(|v|validate_attempt(v,&self.compose_base,&w.project).is_ok());}else if stdout&&line.starts_with("CBGPU_DIAGNOSTICS "){diagnostic=serde_json::from_str::<Value>(&line[18..]).ok().map(|v|redactor.json(&v));}else{logs.push_str(&redactor.text(&line));logs.push('\n');if logs.len()>200000{let mut at=logs.len()-200000;while !logs.is_char_boundary(at){at+=1}logs.drain(..at);}}api.store.logs_update(w,&logs,diagnostic.as_ref(),attempt.as_ref()).await?;},Some(Err(()))=>{failure=Some("Worker output exceeded its safe line limit");break},None=>open=false}},_=tick.tick()=>{if tokio::time::Instant::now()>deadline{failure=Some("Worker timed out; persistent data retained");break}
            if status.is_none(){status=child.try_wait()?;}
            if status.is_none()&&!matches!(tokio::time::timeout(Duration::from_secs(6),admitted(&api,w)).await,Ok(Ok(true))){failure=Some("Execution permission changed or operation cancelled; previous production retained");break}}}
            if status.is_some() && !open {
                break;
            }
        }
        if failure.is_some() {
            #[cfg(unix)]
            if let Some(pid) = child.id() {
                unsafe {
                    libc::kill(-(pid as i32), libc::SIGTERM);
                }
            }
            let _ = child.kill().await;
        }
        out.abort();
        err.abort();
        let success = failure.is_none() && status.is_some_and(|s| s.success());
        if !success {
            if front {
                self.remove_front(w.deployment.as_ref().unwrap(), true)
                    .await?;
            }
            return Err(fail(
                failure.unwrap_or(if front {
                    "Build failed; previous production retained"
                } else {
                    "Compose failed; containers and persistent volumes retained. See diagnostics."
                }),
                503,
            ));
        }
        let result =
            result.ok_or_else(|| fail("Native worker returned no validated result", 503))?;
        if front {
            validate_front(&result, &api.store.data, w.deployment.as_ref().unwrap())?;
            if result["kind"] == "next" {
                let expected = format!("cbfront-run-{}", w.deployment.as_ref().unwrap());
                let raw = self
                    .docker(&[
                        "inspect".into(),
                        "--format".into(),
                        "{{json .}}".into(),
                        expected,
                    ])
                    .await?;
                let found: Value = serde_json::from_str(&raw)?;
                if found["Config"]["Labels"]["cbgpu.frontend.runtime"] != "true"
                    || found["State"]["Running"] != true
                    || !found["NetworkSettings"]["Ports"]
                        .as_object()
                        .is_some_and(|ports| {
                            ports.values().any(|bindings| {
                                bindings.as_array().is_some_and(|bindings| {
                                    bindings.iter().any(|binding| {
                                        binding["HostIp"] == "127.0.0.1"
                                            && binding["HostPort"]
                                                .as_str()
                                                .and_then(|p| p.parse::<u64>().ok())
                                                == result["port"].as_u64()
                                    })
                                })
                            })
                        })
                {
                    return Err(fail("Runtime ownership/health mismatch", 503));
                }
            }
        } else {
            validate_attempt(&result, &self.compose_base, &w.project)?;
        }
        Ok(Some(result))
    }
}
pub fn validate_front(result: &Value, data: &Path, id: &str) -> Result<()> {
    let directory = data.join(id);
    if result["directory"].as_str() != directory.to_str() || !valid_id(id) {
        return Err(fail("Worker directory ownership mismatch", 503));
    }
    match result["kind"].as_str() {
        Some("static") if result["public"].as_str() == directory.join("public").to_str() => Ok(()),
        Some("next")
            if result["container"] == format!("cbfront-run-{id}")
                && result["port"]
                    .as_u64()
                    .is_some_and(|v| (1024..=65535).contains(&v)) =>
        {
            Ok(())
        }
        _ => Err(fail("Invalid native deployment result", 503)),
    }
}
pub fn validate_attempt(result: &Value, base: &Path, id: &str) -> Result<PathBuf> {
    if !valid_id(id) || result["namespace"] != format!("cbgpu-compose-{id}") {
        return Err(fail("Compose namespace ownership mismatch", 503));
    }
    let file = PathBuf::from(
        result["file"]
            .as_str()
            .ok_or_else(|| fail("Managed Compose file missing", 503))?,
    );
    let parent = base.join(id);
    if !file.starts_with(&parent)
        || file.file_name().and_then(|v| v.to_str()) != Some("compose-managed.json")
        || file
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err(fail("Compose file ownership mismatch", 503));
    }
    if file.exists() && !file.canonicalize()?.starts_with(parent.canonicalize()?) {
        return Err(fail("Compose file escapes private project storage", 503));
    }
    Ok(file)
}
fn parse_remote_head(text: &str, branch: &str) -> Option<String> {
    let mut lines = text.lines();
    let line = lines.next()?;
    if lines.next().is_some() {
        return None;
    }
    let fields: Vec<_> = line.split_whitespace().collect();
    if fields.len() != 2
        || fields[1] != format!("refs/heads/{branch}")
        || ![40, 64].contains(&fields[0].len())
        || !fields[0].bytes().all(|b| b.is_ascii_hexdigit())
    {
        return None;
    }
    Some(fields[0].to_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn compose_actor_proof_cannot_fall_back_or_change_when_scoped() {
        let owner = "10000000-0000-4000-8000-000000000001";
        let editor = "10000000-0000-4000-8000-000000000002";
        assert_eq!(
            compose_actor(&json!({}), &json!({}), owner, false),
            Some(owner)
        );
        assert!(compose_actor(&json!({}), &json!({}), owner, true).is_none());
        assert!(
            compose_actor(
                &json!({"requested_by":owner}),
                &json!({"deploy_requested_by":editor}),
                owner,
                true
            )
            .is_none()
        );
        assert!(
            compose_actor(
                &json!({"requested_by":"invalid"}),
                &json!({"deploy_requested_by":"invalid"}),
                owner,
                true
            )
            .is_none()
        );
        assert_eq!(
            compose_actor(
                &json!({"requested_by":editor,"requested_at":"one"}),
                &json!({"deploy_requested_by":editor,"deploy_requested_at":"one"}),
                owner,
                true
            ),
            Some(editor)
        );
        assert!(
            compose_actor(
                &json!({"requested_by":editor,"requested_at":"one"}),
                &json!({"deploy_requested_by":editor,"deploy_requested_at":"two"}),
                owner,
                true
            )
            .is_none()
        );
    }
    #[test]
    fn remote_head_requires_exact_branch_single_hash_and_no_other_output() {
        let hash = "a".repeat(40);
        assert_eq!(
            parse_remote_head(&format!("{hash}\trefs/heads/main\n"), "main"),
            Some(hash.clone())
        );
        for value in [
            format!("{hash}\trefs/heads/other"),
            format!("{hash}\trefs/heads/main\nsecret"),
            "bad refs/heads/main".into(),
        ] {
            assert!(parse_remote_head(&value, "main").is_none());
        }
    }
    #[test]
    fn malicious_worker_markers_cannot_redirect_cleanup_or_runtime() {
        let id = "10000000-0000-4000-8000-000000000001";
        let data = Path::new("/private/frontend");
        assert!(validate_front(&json!({"kind":"next","directory":data.join(id),"container":"jupiterhub_gpu1","port":22}),data,id).is_err());
        assert!(
            validate_front(
                &json!({"kind":"static","directory":data.join(id),"public":"/etc"}),
                data,
                id
            )
            .is_err()
        );
        assert!(validate_attempt(&json!({"namespace":format!("cbgpu-compose-{id}"),"file":"/etc/compose-managed.json"}),Path::new("/private/compose"),id).is_err());
    }
    #[test]
    fn multiline_and_structured_secrets_are_redacted_without_breaking_json() {
        let r = Redactor::new(&json!({"TOKEN":"secret\nvalue","git_token":"github-private"}));
        assert!(
            !r.text("prefix secret value github-private")
                .contains("github-private")
        );
        let value = r.json(&json!({"output":"secret\nvalue"}));
        assert!(!value.to_string().contains("secret"));
    }
}
