//! CI-gated immutable release orchestration. Not activated by installing the source.
use crate::*;
use rusqlite::{Connection, OpenFlags};
use std::{os::unix::process::CommandExt, process::Child};

#[derive(Clone, Debug, Deserialize)]
pub struct Config {
    pub repositories: BTreeMap<String, String>,
    pub frontend_env: PathBuf,
    pub legacy_web: PathBuf,
    pub runtime_backend: PathBuf,
    pub legacy_repo: PathBuf,
    #[serde(default)]
    pub native_ai_binary: Option<PathBuf>,
}
pub trait Runtime: Host {
    fn execute(
        &mut self,
        args: &[String],
        cwd: Option<&Path>,
        env: Option<&BTreeMap<String, String>>,
        input: Option<&[u8]>,
        seconds: u64,
    ) -> Result<String>;
    fn api(&mut self, route: &str, method: &str, body: Option<&Value>) -> Result<Value>;
    fn build(
        &mut self,
        args: &[String],
        directory: &Path,
        env: &BTreeMap<String, String>,
        log: &Path,
    ) -> Result<()>;
    fn preview(
        &mut self,
        args: &[String],
        env: &BTreeMap<String, String>,
        checks: &[Check],
    ) -> Result<()>;
    fn mail(&mut self, key: &str, id: &str, body: &Value) -> Result<()>;
}
fn argv(values: &[&str]) -> Vec<String> {
    values.iter().map(|s| (*s).to_owned()).collect()
}
fn epoch() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
fn event_id() -> String {
    format!(
        "{}-{}-{}",
        epoch(),
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .subsec_nanos()
    )
}
fn error(message: &str) -> String {
    message.to_owned()
}
fn capture(
    args: &[String],
    cwd: Option<&Path>,
    env: Option<&BTreeMap<String, String>>,
    input: Option<&[u8]>,
    seconds: u64,
) -> Result<(bool, String)> {
    let dir = tempfile::tempdir().map_err(|_| error("Cannot create private capture"))?;
    let path = dir.path().join("output");
    let f = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)
        .map_err(|_| error("Cannot create capture"))?;
    let mut cmd = Command::new(args.first().ok_or("Missing executable")?);
    cmd.args(&args[1..])
        .process_group(0)
        .stdout(f.try_clone().map_err(|_| error("Cannot clone capture"))?)
        .stderr(f);
    if let Some(cwd) = cwd {
        cmd.current_dir(cwd);
    }
    if let Some(env) = env {
        cmd.env_clear().envs(env);
    }
    cmd.stdin(if input.is_some() {
        Stdio::piped()
    } else {
        Stdio::null()
    });
    let mut child = cmd.spawn().map_err(|_| error("Cannot start process"))?;
    if let Some(bytes) = input
        && let Some(mut stdin) = child.stdin.take()
        && stdin.write_all(bytes).is_err()
    {
        kill_group(&mut child);
        return Err(error("Cannot send process input"));
    }
    let deadline = Instant::now() + Duration::from_secs(seconds);
    let success = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status.success(),
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(20)),
            _ => {
                kill_group(&mut child);
                break false;
            }
        }
    };
    let mut bytes = Vec::new();
    fs::File::open(path)
        .and_then(|f| f.take(16 * 1024 * 1024).read_to_end(&mut bytes))
        .map_err(|_| error("Cannot read captured output"))?;
    Ok((success, String::from_utf8_lossy(&bytes).into_owned()))
}
fn kill_group(child: &mut Child) {
    unsafe {
        libc::kill(-(child.id() as i32), libc::SIGTERM);
    }
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        if child.try_wait().ok().flatten().is_some() {
            return;
        }
        thread::sleep(Duration::from_millis(20));
    }
    unsafe {
        libc::kill(-(child.id() as i32), libc::SIGKILL);
    }
    let _ = child.wait();
}
pub struct SystemRuntime;
impl Host for SystemRuntime {
    fn reload(&mut self) -> Result<()> {
        self.execute(
            &argv(&["systemctl", "--user", "daemon-reload"]),
            None,
            None,
            None,
            120,
        )
        .map(|_| ())
    }
    fn restart(&mut self, name: &str) -> Result<()> {
        if !unit_name(name) {
            return Err(error("Invalid systemd unit"));
        }
        self.execute(
            &argv(&["systemctl", "--user", "restart", name]),
            None,
            None,
            None,
            90,
        )
        .map(|_| ())
    }
    fn health(&mut self, check: &Check, recovery: bool) -> Result<()> {
        let mut check = check.clone();
        if recovery {
            check.commit = None;
            if check.url.ends_with("/api/health") {
                check.url = check.url.trim_end_matches("api/health").into()
            }
            if check.url == "http://127.0.0.1:3510/health" {
                return self
                    .execute(
                        &argv(&["systemctl", "--user", "is-active", "cbgpu-deploy.service"]),
                        None,
                        None,
                        None,
                        30,
                    )
                    .map(|_| ());
            }
        }
        let url = reqwest::Url::parse(&check.url).map_err(|_| error("Invalid health URL"))?;
        if url.scheme() != "http" || url.host_str() != Some("127.0.0.1") {
            return Err(error("Health probe must stay on loopback"));
        }
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(5))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| error("Cannot create health client"))?;
        let deadline = Instant::now() + Duration::from_secs(45);
        while Instant::now() < deadline {
            let mut req = client.get(url.clone());
            if let Some(host) = &check.host {
                req = req.header("Host", host);
            }
            if let Ok(response) = req.send()
                && response.status().as_u16() == 200
            {
                if let Some(commit) = &check.commit {
                    if response
                        .json::<Value>()
                        .ok()
                        .is_some_and(|v| v["commit"] == *commit)
                    {
                        return Ok(());
                    }
                } else {
                    return Ok(());
                }
            }
            thread::sleep(Duration::from_secs(1));
        }
        Err(error("Release health check failed"))
    }
}
impl Runtime for SystemRuntime {
    fn execute(
        &mut self,
        args: &[String],
        cwd: Option<&Path>,
        env: Option<&BTreeMap<String, String>>,
        input: Option<&[u8]>,
        seconds: u64,
    ) -> Result<String> {
        let (ok, text) = capture(args, cwd, env, input, seconds)?;
        if !ok {
            return Err(error("Command failed; production values omitted"));
        }
        Ok(text.trim().into())
    }
    fn api(&mut self, route: &str, method: &str, body: Option<&Value>) -> Result<Value> {
        let mut args = argv(&["gh", "api", route, "--method", method]);
        let bytes = body
            .map(serde_json::to_vec)
            .transpose()
            .map_err(|_| error("Cannot encode GitHub request"))?;
        if body.is_some() {
            args.extend(argv(&["--input", "-"]));
        }
        let text = self.execute(&args, None, None, bytes.as_deref(), 45)?;
        if text.is_empty() {
            return Ok(Value::Null);
        }
        serde_json::from_str(&text).map_err(|_| error("Invalid GitHub API response"))
    }
    fn build(
        &mut self,
        args: &[String],
        directory: &Path,
        env: &BTreeMap<String, String>,
        log: &Path,
    ) -> Result<()> {
        let result = capture(args, Some(directory), Some(env), None, 1800);
        match result {
            Ok((ok, text)) => {
                let safe = redact(&text, env);
                let mut f = OpenOptions::new()
                    .append(true)
                    .create(true)
                    .mode(0o600)
                    .open(log)
                    .map_err(|_| error("Cannot open private build log"))?;
                let start = safe
                    .char_indices()
                    .map(|(i, _)| i)
                    .find(|i| *i >= safe.len().saturating_sub(2_000_000))
                    .unwrap_or(0);
                f.write_all(&safe.as_bytes()[start..])
                    .map_err(|_| error("Cannot write private build log"))?;
                if ok {
                    Ok(())
                } else {
                    Err(error("Build or validation failed; inspect private log"))
                }
            }
            Err(_) => Err(error(
                "Build timed out or could not start; production unchanged",
            )),
        }
    }
    fn preview(
        &mut self,
        args: &[String],
        env: &BTreeMap<String, String>,
        checks: &[Check],
    ) -> Result<()> {
        let output = tempfile::tempfile().map_err(|_| error("Cannot create preview output"))?;
        let mut child = Command::new(&args[0])
            .args(&args[1..])
            .env_clear()
            .envs(env)
            .process_group(0)
            .stdout(
                output
                    .try_clone()
                    .map_err(|_| error("Cannot clone preview output"))?,
            )
            .stderr(output)
            .spawn()
            .map_err(|_| error("Cannot start preview"))?;
        let result = checks
            .iter()
            .try_for_each(|check| self.health(check, false));
        kill_group(&mut child);
        result
    }
    fn mail(&mut self, key: &str, id: &str, body: &Value) -> Result<()> {
        let response = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(15))
            .build()
            .map_err(|_| error("Cannot create notification client"))?
            .post("https://api.resend.com/emails")
            .bearer_auth(key)
            .header("Idempotency-Key", format!("cbgpu-delivery-{id}"))
            .json(body)
            .send()
            .map_err(|_| error("Notification provider unavailable"))?;
        if !response.status().is_success()
            || response
                .json::<Value>()
                .ok()
                .and_then(|v| v["id"].as_str().map(str::to_owned))
                .is_none()
        {
            return Err(error("Notification provider did not acknowledge message"));
        }
        Ok(())
    }
}

pub fn busy_backend(runtime: &Path) -> Result<bool> {
    let file = runtime.join("private/frontend-deployments/deploy.sqlite");
    if !file.exists() {
        return Ok(false);
    }
    let db = Connection::open_with_flags(file, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|_| error("Cannot read user build state"))?;
    for sql in [
        "SELECT count(*) FROM deployments WHERE status IN ('queued','building')",
        "SELECT count(*) FROM compose_states WHERE status='building'",
    ] {
        match db.query_row(sql, [], |row| row.get::<_, i64>(0)) {
            Ok(count) if count > 0 => return Ok(true),
            Ok(_) => (),
            Err(rusqlite::Error::SqliteFailure(_, Some(message)))
                if message.starts_with("no such table:") => {}
            Err(_) => return Err(error("Cannot validate active user builds")),
        }
    }
    Ok(false)
}
pub fn backup_sqlite(root: &Path, runtime: &Path, sha: &str) -> Result<()> {
    if !valid_sha(sha) {
        return Err(error("Invalid backup commit"));
    }
    for name in [
        "developer.sqlite",
        "frontend-deployments/deploy.sqlite",
        "careers/applications.sqlite",
        "ai/queue.sqlite",
    ] {
        let src = runtime.join("private").join(name);
        if !src.exists() {
            continue;
        }
        let dst = root.join("snapshots").join(sha).join(name);
        fs::create_dir_all(dst.parent().ok_or("Missing snapshot parent")?)
            .map_err(|_| error("Cannot create snapshot directory"))?;
        let db = Connection::open_with_flags(src, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|_| error("Cannot open source database"))?;
        db.backup(
            rusqlite::MAIN_DB,
            &dst,
            None::<fn(rusqlite::backup::Progress)>,
        )
        .map_err(|_| error("Cannot snapshot SQLite data"))?;
    }
    Ok(())
}
fn environment(file: &Path) -> Result<BTreeMap<String, String>> {
    let mut env: BTreeMap<_, _> = std::env::vars().collect();
    for line in fs::read_to_string(file)
        .map_err(|_| error("Cannot read private runtime environment"))?
        .lines()
    {
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        if let Some((k, v)) = line.split_once('=')
            && k.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
        {
            env.insert(k.to_owned(), v.to_owned());
        }
    }
    Ok(env)
}

/// SQLx accepts libpq options in the URL; force read-only and discard conflicting options.
fn readonly_careers_database(value: &str) -> Result<String> {
    let mut url =
        reqwest::Url::parse(value).map_err(|_| error("Invalid private careers database URI"))?;
    if !["postgres", "postgresql"].contains(&url.scheme()) {
        return Err(error("Careers preview requires PostgreSQL"));
    }
    let pairs: Vec<_> = url
        .query_pairs()
        .filter(|(key, _)| key != "options")
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    url.set_query(None);
    url.query_pairs_mut()
        .extend_pairs(pairs)
        .append_pair("options", "-c default_transaction_read_only=on");
    Ok(url.to_string())
}

pub struct Agent<R> {
    pub root: PathBuf,
    pub settings: PathBuf,
    pub config: Config,
    pub state: Value,
    pub runtime: R,
}
impl<R: Runtime> Agent<R> {
    pub fn save(&self) -> Result<()> {
        atomic(
            &self.root.join("state.json"),
            &serde_json::to_vec_pretty(&self.state)
                .map_err(|_| error("Cannot encode delivery state"))?,
        )
    }
    fn record(&self, event: &str, details: Value) -> Result<()> {
        fs::create_dir_all(&self.root).map_err(|_| error("Cannot create event directory"))?;
        let mut fields = details.as_object().cloned().unwrap_or_default();
        fields.insert("details".into(), details);
        fields.insert(
            "at".into(),
            json!(chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()),
        );
        fields.insert("event".into(), json!(event));
        let data = Value::Object(fields);
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .open(self.root.join("events.jsonl"))
            .map_err(|_| error("Cannot record delivery event"))?;
        writeln!(file, "{data}").map_err(|_| error("Cannot append event"))?;
        println!("{data}");
        Ok(())
    }
    fn repo_candidate(&mut self, repo: &str) -> Result<(String, String)> {
        if !allowed_repo(repo) {
            return Err(error("Repository is not allowed"));
        }
        let reference =
            self.runtime
                .api(&format!("repos/{repo}/git/ref/heads/main"), "GET", None)?;
        let sha = reference["object"]["sha"]
            .as_str()
            .filter(|s| valid_sha(s))
            .ok_or("Invalid main commit")?;
        let runs=self.runtime.api(&format!("repos/{repo}/actions/workflows/ci.yml/runs?branch=main&event=push&head_sha={sha}&per_page=5"),"GET",None)?;
        ci_gate(repo, &reference, &runs)
    }
    fn paths(&mut self, repo: &str, previous: Option<&str>, sha: &str) -> Result<Vec<String>> {
        match previous {
            Some(old) if valid_sha(old) => changed_paths(&self.runtime.api(
                &format!("repos/{repo}/compare/{old}...{sha}"),
                "GET",
                None,
            )?),
            Some(_) => Err(error("Invalid previous commit")),
            None => Ok(vec![]),
        }
    }
    fn checkout(&mut self, component: &str, repo: &str, sha: &str) -> Result<PathBuf> {
        if !["web", "backend"].contains(&component) || !allowed_repo(repo) || !valid_sha(sha) {
            return Err(error("Invalid release identity"));
        }
        let mirror = self.root.join("mirrors").join(format!("{component}.git"));
        let destination = self
            .root
            .join("releases")
            .join(component)
            .join(sha)
            .join(component);
        if destination.exists() {
            let head = self.runtime.execute(
                &argv(&["git", "rev-parse", "HEAD"]),
                Some(&destination),
                None,
                None,
                120,
            )?;
            if head != sha {
                return Err(error("Release directory commit mismatch"));
            }
            return Ok(destination);
        }
        fs::create_dir_all(mirror.parent().unwrap()).map_err(|_| error("Cannot create mirrors"))?;
        if !mirror.exists() {
            self.runtime.execute(
                &argv(&[
                    "git",
                    "init",
                    "--bare",
                    mirror.to_str().ok_or("Invalid mirror path")?,
                ]),
                None,
                None,
                None,
                120,
            )?;
        }
        let mirror = mirror.to_str().ok_or("Invalid mirror path")?;
        self.runtime.execute(
            &argv(&[
                "git",
                "--git-dir",
                mirror,
                "fetch",
                "--depth=1",
                &format!("https://github.com/{repo}.git"),
                sha,
            ]),
            None,
            None,
            None,
            180,
        )?;
        fs::create_dir_all(destination.parent().unwrap())
            .map_err(|_| error("Cannot create immutable release"))?;
        self.runtime.execute(
            &argv(&[
                "git",
                "--git-dir",
                mirror,
                "worktree",
                "add",
                "--detach",
                destination.to_str().ok_or("Invalid release path")?,
                sha,
            ]),
            None,
            None,
            None,
            120,
        )?;
        Ok(destination)
    }
    fn deploy_web(
        &mut self,
        sha: &str,
        repo: &str,
        log: &Path,
    ) -> Result<(PathBuf, BTreeMap<String, String>)> {
        let directory = self.checkout("web", repo, sha)?;
        let mut env = environment(&self.config.frontend_env)?;
        env.insert("NEXT_BUILD_DIR".into(), ".next-production".into());
        env.insert("CBGPU_RELEASE_SHA".into(), sha.into());
        env.insert("NEXT_TELEMETRY_DISABLED".into(), "1".into());
        env.entry("CBGPU_ANDROID_RELEASE_PATH".into())
            .or_insert_with(|| {
                self.config
                    .legacy_web
                    .join("public/downloads/android-release.json")
                    .to_string_lossy()
                    .into_owned()
            });
        let apk = self.config.legacy_web.join("public/downloads/cbgpu.apk");
        let target = directory.join("public/downloads/cbgpu.apk");
        if apk.exists() && !target.exists() {
            fs::create_dir_all(target.parent().unwrap())
                .map_err(|_| error("Cannot create downloads directory"))?;
            std::os::unix::fs::symlink(apk, target)
                .map_err(|_| error("Cannot link existing app download"))?;
        }
        if !directory.join(".delivery-built").exists() {
            self.runtime.build(
                &argv(&["npm", "ci", "--no-audit", "--no-fund"]),
                &directory,
                &env,
                log,
            )?;
            self.runtime
                .build(&argv(&["npm", "run", "build"]), &directory, &env, log)?;
            atomic(&directory.join(".delivery-built"), sha.as_bytes())?;
        }
        let mut previous: Vec<_> = fs::read_dir(self.root.join("releases/web"))
            .map_err(|_| error("Cannot list previous releases"))?
            .filter_map(|e| e.ok())
            .map(|e| e.path().join("web"))
            .filter(|p| *p != directory && p.join(".delivery-built").exists())
            .collect();
        previous
            .sort_by_key(|p| std::cmp::Reverse(fs::metadata(p).and_then(|m| m.modified()).ok()));
        retain_web_assets(&directory, &previous)?;
        let saved = env
            .iter()
            .filter(|(k, _)| {
                k.starts_with("NEXT_PUBLIC_")
                    || [
                        "SUPABASE_INTERNAL_URL",
                        "SUPABASE_SERVICE_ROLE_KEY",
                        "EDUSET_TOKEN_SECRET",
                        "RESEND_API_KEY",
                        "CBGPU_EMAIL_FROM",
                        "CBGPU_TRAFFIC_SECRET",
                        "CBGPU_STUDY_AUTOMATION_SECRET",
                        "SPOTIFY_CLIENT_ID",
                        "SPOTIFY_CLIENT_SECRET",
                        "SPOTIFY_REDIRECT_URI",
                        "CBGPU_ACCOUNTS_API_URL",
                        "CBGPU_MULTISCHOOL_V1",
                        "CBGPU_SCHOOLS_API_URL",
                        "CBGPU_TIMETABLE_WARM_SECRET_FILE",
                        "CBGPU_PUBLIC_CONTENT_API_URL",
                        "CBGPU_AD_REVENUE_ENABLED",
                        "CBGPU_AD_REVENUE_API_URL",
                        "CBGPU_ANDROID_RELEASE_PATH",
                        "CBGPU_BACKEND_PROXY_URL",
                        "CBGPU_SESSION_ORACLE_MODE",
                        "CBGPU_INTERNAL_NETWORK_MODE",
                        "CBGPU_PUBLIC_BROWSING_V1",
                    ]
                    .contains(&k.as_str())
            })
            .map(|(k, v)| format!("{k}={v}\n"))
            .collect::<String>();
        atomic(&directory.join(".env.production.local"), saved.as_bytes())?;
        let next = directory.join("node_modules/next/dist/bin/next");
        let mut checks = vec![Check {
            url: "http://127.0.0.1:3002/api/health".into(),
            commit: Some(sha.into()),
            host: None,
        }];
        for host in [
            "cbgpu.com",
            "app.cbgpu.com",
            "docs.cbgpu.com",
            "arxiv.cbgpu.com",
        ] {
            checks.push(Check {
                url: "http://127.0.0.1:3002/".into(),
                commit: None,
                host: Some(host.into()),
            })
        }
        self.runtime.preview(
            &argv(&[
                "/usr/bin/node",
                next.to_str().ok_or("Invalid Next path")?,
                "start",
                directory.to_str().ok_or("Invalid release path")?,
                "--hostname",
                "127.0.0.1",
                "--port",
                "3002",
            ]),
            &env,
            &checks,
        )?;
        let unit = fs::read_to_string(self.settings.join("cbgpu-frontend.service"))
            .map_err(|_| error("Cannot read frontend service"))?;
        let unit = unit
            .lines()
            .map(|l| {
                if l.starts_with("EnvironmentFile=") {
                    format!(
                        "EnvironmentFile={}/.env.production.local",
                        directory.display()
                    )
                } else {
                    l.to_owned()
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        let unit = unit_update(
            &unit,
            &directory,
            &format!(
                "/usr/bin/node {} start --hostname 127.0.0.1 --port 3000",
                next.display()
            ),
            &BTreeMap::from([
                ("NEXT_BUILD_DIR".into(), ".next-production".into()),
                ("CBGPU_RELEASE_SHA".into(), sha.into()),
            ]),
        )?;
        let before = switch(
            &self.root,
            &self.settings,
            &BTreeMap::from([("cbgpu-frontend.service".into(), unit)]),
            vec![Check {
                url: "http://127.0.0.1:3000/api/health".into(),
                commit: Some(sha.into()),
                host: None,
            }],
            &mut self.runtime,
        )?;
        Ok((directory, before))
    }
    fn deploy_backend(
        &mut self,
        sha: &str,
        repo: &str,
        previous: Option<&str>,
        log: &Path,
        approved: bool,
    ) -> Result<(PathBuf, BTreeMap<String, String>)> {
        let paths = self.paths(repo, previous, sha)?;
        if paths.iter().any(|p| protected(p)) && !approved {
            return Err(error(
                "DEFERRED: Infrastructure/schema changes require separate rollout",
            ));
        }
        // AI has independent durable queues and GPU duties. A CI-passed commit or
        // an ELF path is not proof that its old writer has been fenced/imported.
        if previous.is_none()
            || paths.iter().any(|path| {
                path.starts_with("ai/")
                    || path.starts_with("rust/crates/cbgpu-ai/")
                    || path.starts_with("rust/crates/cbgpu-ai-")
            })
        {
            return Err(error(
                "DEFERRED: AI changes require a separately verified fenced data handoff; the active immutable AI release is preserved",
            ));
        }
        if busy_backend(&self.config.runtime_backend)? {
            return Err(error("DEFERRED: User deployment is running"));
        }
        // Careers has already migrated to native PostgreSQL. Never revert it to Python.
        let careers_unit = fs::read_to_string(self.settings.join("cbgpu-careers.service"))
            .map_err(|_| error("DEFERRED: Existing native careers unit missing"))?;
        if careers_unit.contains("careers/server.py") {
            return Err(error(
                "DEFERRED: Careers native data handoff must be completed separately",
            ));
        }
        let careers_env_path = careers_unit
            .lines()
            .find_map(|line| line.strip_prefix("EnvironmentFile="))
            .ok_or("DEFERRED: Native careers private environment missing")?;
        let careers_env_path = PathBuf::from(careers_env_path);
        if !careers_env_path.is_absolute() || !careers_env_path.is_file() {
            return Err(error(
                "DEFERRED: Native careers private environment unavailable",
            ));
        }
        let mut careers_env = environment(&careers_env_path)?;
        if !careers_env.contains_key("CAREERS_DATABASE_URL")
            || !careers_env.contains_key("CAREERS_FORM_KEY_PATH")
        {
            return Err(error("DEFERRED: Native careers data connection incomplete"));
        }
        let directory = self.checkout("backend", repo, sha)?;
        let runtime = &self.config.runtime_backend;
        for name in ["private", ".env", "functions.env", "backups", "releases"] {
            let src = runtime.join(name);
            let dst = directory.join(name);
            if src.exists() && !dst.exists() {
                std::os::unix::fs::symlink(src, dst)
                    .map_err(|_| error("Cannot link persistent runtime path"))?;
            }
        }
        for (name, src) in [
            (
                "FrontEnd",
                self.config
                    .legacy_web
                    .parent()
                    .ok_or("Invalid legacy frontend")?
                    .to_path_buf(),
            ),
            ("Daemon", self.config.legacy_repo.join("Daemon")),
        ] {
            let target = directory.parent().ok_or("Invalid release")?.join(name);
            if src.exists() && !target.exists() {
                std::os::unix::fs::symlink(src, target)
                    .map_err(|_| error("Cannot link legacy runtime dependency"))?;
            }
        }
        let mut env: BTreeMap<_, _> = std::env::vars().collect();
        env.insert("NEXT_TELEMETRY_DISABLED".into(), "1".into());
        if !directory.join("developer/node_modules").exists() {
            self.runtime.build(
                &argv(&["npm", "ci", "--no-audit", "--no-fund"]),
                &directory.join("developer"),
                &env,
                log,
            )?;
        }
        if directory.join("package.json").exists() {
            self.runtime.build(
                &argv(&["npm", "ci", "--ignore-scripts", "--no-audit", "--no-fund"]),
                &directory,
                &env,
                log,
            )?;
            self.runtime.build(
                &argv(&["npm", "run", "build:browser"]),
                &directory,
                &env,
                log,
            )?;
        }
        self.runtime.build(
            &argv(&["node", "scripts/verify-mail-queue.mts"]),
            &directory,
            &env,
            log,
        )?;
        // Build the pinned native careers service; preview shares no write capabilities.
        let binary_target = directory.join(".delivery-native-target");
        let mut cargo_env = env.clone();
        cargo_env.insert(
            "PATH".into(),
            format!(
                "{}/.cargo/bin:{}",
                std::env::var("HOME").unwrap_or_else(|_| "/home/cbgpu".into()),
                cargo_env.get("PATH").cloned().unwrap_or_default()
            ),
        );
        cargo_env.insert(
            "CARGO_TARGET_DIR".into(),
            binary_target.display().to_string(),
        );
        cargo_env.insert("CARGO_BUILD_JOBS".into(), "2".into());
        self.runtime.build(
            &argv(&[
                PathBuf::from(std::env::var_os("HOME").ok_or("HOME missing for native compiler")?)
                    .join(".cargo/bin/cargo")
                    .to_str()
                    .ok_or("Invalid Cargo path")?,
                "build",
                "--locked",
                "--release",
                "--manifest-path",
                directory
                    .join("rust/Cargo.toml")
                    .to_str()
                    .ok_or("Invalid native manifest")?,
                "-p",
                "cbgpu-careers",
            ]),
            &directory,
            &cargo_env,
            log,
        )?;
        let careers_binary = directory.join(".delivery-bin/cbgpu-careers");
        atomic(
            &careers_binary,
            &fs::read(binary_target.join("release/cbgpu-careers"))
                .map_err(|_| error("Native careers build output missing"))?,
        )?;
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&careers_binary, fs::Permissions::from_mode(0o700))
            .map_err(|_| error("Cannot mark native careers executable"))?;
        careers_env.insert(
            "CAREERS_PUBLIC_DIR".into(),
            directory.join("careers/public").display().to_string(),
        );
        careers_env.insert(
            "CAREERS_ROLES_PATH".into(),
            directory.join("careers/roles.json").display().to_string(),
        );
        careers_env.insert("CBGPU_RELEASE_SHA".into(), sha.into());
        let careers_release_env = directory.join(".careers.production.env");
        let saved = careers_env
            .iter()
            .filter(|(k, _)| k.starts_with("CAREERS_") || k.as_str() == "CBGPU_RELEASE_SHA")
            .map(|(k, v)| format!("{k}={v}\n"))
            .collect::<String>();
        atomic(&careers_release_env, saved.as_bytes())?;
        let mut preview_env = careers_env.clone();
        preview_env.insert("CAREERS_ENABLE_OUTBOX".into(), "0".into());
        preview_env.insert("CAREERS_LISTEN".into(), "127.0.0.1:13530".into());
        preview_env.remove("CAREERS_MAIL_ENV");
        preview_env.remove("RESEND_API_KEY");
        preview_env.insert(
            "CAREERS_DATABASE_URL".into(),
            readonly_careers_database(
                careers_env
                    .get("CAREERS_DATABASE_URL")
                    .ok_or("Missing careers database")?,
            )?,
        );
        self.runtime.preview(
            &argv(&[
                careers_binary.to_str().ok_or("Invalid careers binary")?,
                "serve",
            ]),
            &preview_env,
            &[Check {
                url: "http://127.0.0.1:13530/health".into(),
                commit: Some(sha.into()),
                host: None,
            }],
        )?;
        if busy_backend(runtime)? {
            return Err(error("DEFERRED: User build started during validation"));
        }
        backup_sqlite(&self.root, runtime, sha)?;
        let mut functions = RuntimeFunctions {
            runtime: &mut self.runtime,
        };
        let synced = sync_functions(&directory, runtime, &paths, sha, &mut functions)?;
        if !synced.is_empty() {
            self.record("functions_synced", json!({"commit":sha,"files":synced}))?;
        }
        let definitions = [
            (
                "careers",
                "cbgpu-careers.service",
                format!("{} serve", careers_binary.display()),
                3530,
            ),
            (
                "developer",
                "cbgpu-developer-portal.service",
                format!("/usr/bin/node {}/developer/server.mts", directory.display()),
                3400,
            ),
            (
                "deploy",
                "cbgpu-deploy.service",
                format!("/usr/bin/node {}/deploy/server.mts", directory.display()),
                3510,
            ),
        ];
        let mut units = BTreeMap::new();
        let mut checks = Vec::new();
        for (prefix, name, execution, port) in definitions {
            if previous.is_some()
                && !paths.iter().any(|p| {
                    p.starts_with(&format!("{prefix}/"))
                        || prefix == "careers" && p.starts_with("rust/crates/cbgpu-careers/")
                })
            {
                continue;
            }
            let mut original = fs::read_to_string(self.settings.join(name))
                .map_err(|_| error("Cannot read backend unit"))?;
            if prefix == "careers" {
                original = original
                    .lines()
                    .map(|line| {
                        if line.starts_with("EnvironmentFile=") {
                            format!("EnvironmentFile={}", careers_release_env.display())
                        } else {
                            line.to_owned()
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
            }
            let mut extra = BTreeMap::from([("CBGPU_RELEASE_SHA".into(), sha.into())]);
            if prefix == "developer" {
                extra.insert("CBGPU_PORTAL_ROOT".into(), directory.display().to_string());
                extra.insert(
                    "CBGPU_PORTAL_REPO".into(),
                    self.config.legacy_repo.display().to_string(),
                );
            }
            units.insert(
                name.into(),
                unit_update(
                    &original,
                    &if prefix == "deploy" {
                        directory.join("deploy")
                    } else {
                        directory.clone()
                    },
                    &execution,
                    &extra,
                )?,
            );
            checks.push(Check {
                url: format!("http://127.0.0.1:{port}/health"),
                commit: Some(sha.into()),
                host: None,
            });
        }
        let before = if units.is_empty() {
            BTreeMap::new()
        } else {
            switch(
                &self.root,
                &self.settings,
                &units,
                checks,
                &mut self.runtime,
            )?
        };
        Ok((directory, before))
    }
    fn job(
        &self,
        id: &str,
        component: &str,
        sha: &str,
        status: &str,
        message: Option<&str>,
    ) -> Result<()> {
        let file = self.config.runtime_backend.join("private/developer.sqlite");
        if !file.exists() {
            return Ok(());
        }
        let db = Connection::open(file).map_err(|_| error("Cannot open operator job journal"))?;
        db.busy_timeout(Duration::from_secs(10))
            .map_err(|_| error("Cannot configure journal timeout"))?;
        if status == "running" {
            db.execute("INSERT OR IGNORE INTO jobs(id,status,started_at,commit_sha,action)VALUES(?1,?2,datetime('now'),?3,?4)",rusqlite::params![id,status,sha,format!("auto:{component}")]).map_err(|_|error("Cannot record running delivery"))?;
        } else {
            db.execute(
                "UPDATE jobs SET status=?1,finished_at=datetime('now'),error=?2 WHERE id=?3",
                rusqlite::params![status, message, id],
            )
            .map_err(|_| error("Cannot finish delivery journal"))?;
        }
        Ok(())
    }
    fn notify(&self, component: &str, sha: &str, status: &str, id: &str) -> Result<()> {
        if !safe_relative(id) || id.contains('/') {
            return Err(error("Invalid notification ID"));
        }
        atomic(
            &self.root.join("outbox").join(format!("{id}.json")),
            &serde_json::to_vec(
                &json!({"component":component,"commit":sha,"status":status,"id":id}),
            )
            .map_err(|_| error("Cannot encode notification"))?,
        )
    }
    fn flush_notifications(&mut self) -> Result<()> {
        let env = match environment(&self.config.runtime_backend.join(".env")) {
            Ok(env) => env,
            Err(_) => return Ok(()),
        };
        let Some(key) = env.get("SMTP_PASS").filter(|k| k.starts_with("re_")) else {
            return Ok(());
        };
        let folder = self.root.join("outbox");
        if !folder.exists() {
            return Ok(());
        }
        let mut paths: Vec<_> = fs::read_dir(folder)
            .map_err(|_| error("Cannot read outbox"))?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "json"))
            .collect();
        paths.sort();
        for path in paths.into_iter().take(5) {
            let item = read_json(&path)?;
            let component = item["component"]
                .as_str()
                .filter(|v| ["web", "backend"].contains(v))
                .ok_or("Invalid notification component")?;
            let sha = item["commit"].as_str().unwrap_or("unknown");
            let status = item["status"].as_str().unwrap_or("unknown");
            let id = item["id"].as_str().ok_or("Invalid notification ID")?;
            // JSON escaping alone is insufficient in HTML. Treat status/state as plain text.
            let escape = |s: &str| {
                s.replace('&', "&amp;")
                    .replace('<', "&lt;")
                    .replace('>', "&gt;")
                    .replace('"', "&quot;")
            };
            let short = sha.chars().take(12).collect::<String>();
            let body = json!({"from":"CBGPU Operations <info@cbgpu.com>","to":["info@cbgpu.com"],"subject":format!("[CBGPU 운영] {component} {status} · {short}"),"html":format!("<div style='font-family:Arial,sans-serif;padding:28px;max-width:560px'><h2>CBGPU 배포 상태</h2><p>{}: <b>{}</b></p><p>커밋: {}</p><p>상세 로그는 개발자 페이지에서 확인해주세요.</p><a href='https://developers.cbgpu.com'>개발자 페이지 열기</a></div>",escape(component),escape(status),escape(&short))});
            if self.runtime.mail(key, id, &body).is_err() {
                self.record(
                    "notification_deferred",
                    json!({"component":component,"id":id}),
                )?;
                break;
            }
            fs::remove_file(path).map_err(|_| error("Cannot acknowledge outbox message"))?;
            thread::sleep(Duration::from_millis(600));
        }
        Ok(())
    }
    pub fn recover(&mut self) -> Result<bool> {
        recover(&self.root, &self.settings, &mut self.runtime)
    }
    pub fn approve_backend(&mut self, sha: &str) -> Result<()> {
        let repo = self
            .config
            .repositories
            .get("backend")
            .ok_or("Backend repository missing")?
            .clone();
        let (expected, _) = self.repo_candidate(&repo)?;
        if !valid_sha(sha) || sha != expected {
            return Err(error("Acknowledgement must match current CI-passed main"));
        }
        self.state["components"]["backend"]["approved_infrastructure_sha"] = json!(sha);
        self.state["components"]["backend"]["attempt_at"] = json!(0);
        self.save()?;
        self.record("manual_infrastructure_acknowledged", json!({"commit":sha}))
    }
    pub fn rollback(&mut self, component: &str) -> Result<()> {
        if !["web", "backend"].contains(&component) {
            return Err(error("Unknown rollback component"));
        }
        let current = self.state["components"][component].clone();
        let units: BTreeMap<String, String> =
            serde_json::from_value(current["rollback_units"].clone())
                .map_err(|_| error("No prior unit snapshot"))?;
        if units.is_empty() {
            return Err(error("No previous units"));
        }
        let checks = if component == "web" {
            vec![Check {
                url: "http://127.0.0.1:3000/".into(),
                commit: None,
                host: None,
            }]
        } else {
            units
                .keys()
                .filter_map(|name| match name.as_str() {
                    "cbgpu-developer-portal.service" => Some(3400),
                    "cbgpu-careers.service" => Some(3530),
                    "cbgpu-deploy.service" => Some(3510),
                    "cbgpu-ai.service" => Some(3550),
                    _ => None,
                })
                .map(|port| Check {
                    url: format!("http://127.0.0.1:{port}/health"),
                    commit: None,
                    host: None,
                })
                .collect()
        };
        switch(
            &self.root,
            &self.settings,
            &units,
            checks,
            &mut self.runtime,
        )?;
        let curr = &mut self.state["components"][component];
        curr["blocked_sha"] = current["sha"].clone();
        curr["sha"] = current["previous_sha"].clone();
        curr["release"] = current["previous_release"].clone();
        curr["status"] = json!("rolled_back");
        curr["rollback_units"] = Value::Null;
        self.save()?;
        self.record("manual_rollback", json!({"component":component}))
    }
    pub fn retry(&mut self, component: &str) -> Result<()> {
        if !["web", "backend"].contains(&component) {
            return Err(error("Unknown component"));
        }
        let current = &mut self.state["components"][component];
        if current.is_null() {
            *current = json!({})
        }
        let map = current.as_object_mut().ok_or("Invalid component state")?;
        map.remove("failed_sha");
        map.remove("blocked_sha");
        map.insert("attempt_at".into(), json!(0));
        self.save()
    }
    fn cleanup(&mut self) -> Result<()> {
        let mut references = self.state.to_string();
        if self.settings.exists() {
            for unit in fs::read_dir(&self.settings)
                .map_err(|_| error("Cannot inspect current units"))?
                .filter_map(|e| e.ok())
            {
                if unit_name(&unit.file_name().to_string_lossy()) {
                    references.push_str(
                        &fs::read_to_string(unit.path())
                            .map_err(|_| error("Cannot read current unit reference"))?,
                    )
                }
            }
        }
        for component in ["web", "backend"] {
            let folder = self.root.join("releases").join(component);
            if !folder.exists() {
                continue;
            }
            let mut releases: Vec<_> = fs::read_dir(folder)
                .map_err(|_| error("Cannot enumerate old releases"))?
                .filter_map(|e| e.ok())
                .filter(|e| {
                    e.file_type().is_ok_and(|t| t.is_dir())
                        && valid_sha(&e.file_name().to_string_lossy())
                })
                .map(|e| e.path())
                .collect();
            releases.sort_by_key(|p| {
                std::cmp::Reverse(fs::metadata(p).and_then(|m| m.modified()).ok())
            });
            for release in releases.into_iter().skip(5) {
                let sha = release.file_name().unwrap().to_string_lossy();
                let age = fs::metadata(&release)
                    .and_then(|m| m.modified())
                    .ok()
                    .and_then(|t| t.elapsed().ok())
                    .unwrap_or_default();
                if references.contains(sha.as_ref()) || age < Duration::from_secs(7 * 86400) {
                    continue;
                }
                let worktree = release.join(component);
                if worktree.exists() {
                    self.runtime.execute(
                        &argv(&[
                            "git",
                            "--git-dir",
                            self.root
                                .join("mirrors")
                                .join(format!("{component}.git"))
                                .to_str()
                                .ok_or("Invalid mirror path")?,
                            "worktree",
                            "remove",
                            "--force",
                            worktree.to_str().ok_or("Invalid old worktree")?,
                        ]),
                        None,
                        None,
                        None,
                        180,
                    )?;
                }
                for name in ["FrontEnd", "Daemon"] {
                    let link = release.join(name);
                    if fs::symlink_metadata(&link).is_ok_and(|m| m.file_type().is_symlink()) {
                        fs::remove_file(link)
                            .map_err(|_| error("Cannot remove old runtime link"))?;
                    }
                }
                let _ = fs::remove_dir(&release);
                self.record(
                    "old_release_pruned",
                    json!({"component":component,"commit":sha}),
                )?;
            }
        }
        Ok(())
    }
    pub fn poll(&mut self, only: Option<&str>) -> Result<()> {
        if self.state["paused"] == true {
            self.record("poll_skipped_paused", json!({}))?;
            return Ok(());
        }
        let repos = self.config.repositories.clone();
        for (component, repo) in repos {
            if only.is_some_and(|only| only != component) {
                continue;
            }
            if !["web", "backend"].contains(&component.as_str()) || !allowed_repo(&repo) {
                return Err(error("Repository configuration violates installed policy"));
            }
            if self.state["components"][&component].is_null() {
                self.state["components"][&component] = json!({})
            }
            let candidate = self.repo_candidate(&repo);
            let (sha, ci) = match candidate {
                Ok(v) => v,
                Err(message) => {
                    let current = &mut self.state["components"][&component];
                    if message.starts_with("Waiting for successful") {
                        current["waiting_for"] = json!(message);
                    } else {
                        let errors = current["poll_errors"].as_u64().unwrap_or(0) + 1;
                        current["poll_errors"] = json!(errors);
                        current["poll_error"] = json!(message);
                        if errors >= 3 && current["poll_incident"] != true {
                            current["poll_incident"] = json!(true);
                            let old = current["sha"].as_str().unwrap_or("unknown").to_owned();
                            self.notify(&component, &old, "자동 배포 연결 오류", &event_id())?;
                        }
                    }
                    self.save()?;
                    continue;
                }
            };
            if self.state["components"][&component]["poll_incident"] == true {
                self.notify(&component, &sha, "배포 연결 복구", &event_id())?;
            }
            self.state["components"][&component]["poll_errors"] = json!(0);
            self.state["components"][&component]["poll_incident"] = json!(false);
            self.state["components"][&component]["poll_error"] = Value::Null;
            let current = self.state["components"][&component].clone();
            if current["sha"] == sha
                || current["blocked_sha"] == sha
                || current["failed_sha"] == sha
            {
                continue;
            }
            let previous = current["sha"].as_str().map(str::to_owned);
            let approved = current["approved_infrastructure_sha"] == sha;
            let preflight = (|| {
                if component == "backend" {
                    let paths = self.paths(&repo, previous.as_deref(), &sha)?;
                    if !approved && paths.iter().any(|p| protected(p)) {
                        return Err(error(
                            "Infrastructure/schema changes require separate rollout",
                        ));
                    }
                    if busy_backend(&self.config.runtime_backend)? {
                        return Err(error("Waiting for active user build workers"));
                    }
                }
                if current["attempt_at"].as_u64().unwrap_or(0) > epoch().saturating_sub(120) {
                    return Err(error("Waiting for deployment retry interval"));
                }
                self.cleanup()?;
                if fs2::available_space(&self.root)
                    .map_err(|_| error("Cannot verify free delivery space"))?
                    < 15 * 1024 * 1024 * 1024
                {
                    return Err(error("Less than15GiB free; production unchanged"));
                }
                Ok(())
            })();
            if let Err(message) = preflight {
                self.state["components"][&component]["waiting_for"] = json!(message);
                self.save()?;
                continue;
            }
            let id = event_id();
            let log = self.root.join("logs").join(format!("{id}.log"));
            atomic(&log, b"")?;
            let portal = self
                .config
                .runtime_backend
                .join("private")
                .join(format!("deploy-{id}.log"));
            if portal.parent().is_some_and(Path::exists) && !portal.exists() {
                std::os::unix::fs::symlink(&log, &portal)
                    .map_err(|_| error("Cannot link operator build log"))?;
            }
            {
                let current = &mut self.state["components"][&component];
                current["attempt_at"] = json!(epoch());
                current["status"] = json!("building");
                current["candidate"] = json!(sha);
            }
            self.save()?;
            self.job(&id, &component, &sha, "running", None)?;
            self.record(
                "deploy_started",
                json!({"component":component,"commit":sha,"ci":ci,"id":id}),
            )?;
            let mut deployment: Option<Value> = None;
            let rollout = (|| {
                let created=self.runtime.api(&format!("repos/{repo}/deployments"),"POST",Some(&json!({"ref":sha,"environment":"production","auto_merge":false,"required_contexts":[],"production_environment":true,"description":"Host verified main CI; immutable release and health-gated switch"})))?;
                let id = created["id"].clone();
                if id.is_null() {
                    return Err(error("GitHub deployment ID missing"));
                }
                deployment = Some(id.clone());
                self.runtime.api(
                    &format!("repos/{repo}/deployments/{id}/statuses"),
                    "POST",
                    Some(&json!({"state":"in_progress","log_url":ci})),
                )?;
                if component == "web" {
                    self.deploy_web(&sha, &repo, &log)
                } else {
                    self.deploy_backend(&sha, &repo, previous.as_deref(), &log, approved)
                }
            })();
            match rollout {
                Ok((directory, before)) => {
                    let state = &mut self.state["components"][&component];
                    state["previous_sha"] = current["sha"].clone();
                    state["previous_release"] = current["release"].clone();
                    state["sha"] = json!(sha);
                    state["release"] = json!(directory);
                    state["rollback_units"] = json!(before);
                    state["status"] = json!("healthy");
                    state["deployed_at"] = json!(epoch());
                    state["failed_sha"] = Value::Null;
                    state["candidate"] = Value::Null;
                    state["waiting_for"] = Value::Null;
                    self.save()?;
                    self.job(&id, &component, &sha, "succeeded", None)?;
                    self.record(
                        "deploy_succeeded",
                        json!({"component":component,"commit":sha,"id":id}),
                    )?;
                    self.notify(&component, &sha, "배포 성공", &id)?;
                    if let Some(deployment) = deployment
                        && self.runtime.api(&format!("repos/{repo}/deployments/{deployment}/statuses"),"POST",Some(&json!({"state":"success","environment_url":if component=="web"{"https://cbgpu.com"}else{"https://developers.cbgpu.com"},"log_url":ci}))).is_err(){self.record("reporting_failed",json!({"component":component,"commit":sha}))?;}
                }
                Err(message) => {
                    let deferred = message.starts_with("DEFERRED:");
                    let status = if deferred { "deferred" } else { "failed" };
                    self.state["components"][&component]["status"] = json!(status);
                    self.state["components"][&component]["message"] = json!(message);
                    if !deferred {
                        self.state["components"][&component]["failed_sha"] = json!(sha);
                    }
                    self.save()?;
                    self.job(&id, &component, &sha, status, Some(&message))?;
                    self.record(
                        if deferred {
                            "deploy_deferred"
                        } else {
                            "deploy_failed"
                        },
                        json!({"component":component,"commit":sha,"message":message}),
                    )?;
                    if !deferred {
                        self.notify(&component, &sha, "배포 실패 — 운영 상태 확인 필요", &id)?;
                    }
                    if let Some(deployment) = deployment {
                        let _=self.runtime.api(&format!("repos/{repo}/deployments/{deployment}/statuses"),"POST",Some(&json!({"state":if deferred{"inactive"}else{"failure"},"description":"Host rollout did not complete; inspect private logs"})));
                    }
                }
            }
        }
        self.flush_notifications()?;
        self.state["last_poll"] = json!(epoch());
        self.save()
    }
}
struct RuntimeFunctions<'a, R> {
    runtime: &'a mut R,
}
impl<R: Runtime> FunctionHost for RuntimeFunctions<'_, R> {
    fn restart(&mut self) -> Result<()> {
        let container = std::env::var("CBGPU_FUNCTIONS_CONTAINER")
            .unwrap_or_else(|_| "cbgpu-supabase-edge-functions".into());
        if !container
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b))
        {
            return Err(error("Invalid function container"));
        }
        self.runtime
            .execute(
                &argv(&["docker", "restart", &container]),
                None,
                None,
                None,
                90,
            )
            .map(|_| ())
    }
    fn healthy(&mut self) -> Result<()> {
        let container = std::env::var("CBGPU_FUNCTIONS_CONTAINER")
            .unwrap_or_else(|_| "cbgpu-supabase-edge-functions".into());
        let deadline = Instant::now() + Duration::from_secs(60);
        while Instant::now() < deadline {
            if self.runtime.execute(&argv(&["docker","inspect","--format","{{if .State.Health}}{{.State.Health.Status}}{{else}}{{.State.Status}}{{end}}",&container]),None,None,None,20).ok().is_some_and(|value|matches!(value.as_str(),"healthy"|"running")){return Ok(())}
            thread::sleep(Duration::from_secs(2))
        }
        Err(error("Edge functions did not become healthy"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    #[derive(Default)]
    struct FakeRuntime {
        commands: Vec<Vec<String>>,
        builds: usize,
        restarts: usize,
        api_posts: Vec<Value>,
        fail_build: bool,
        fail_preview: bool,
        fail_live: bool,
        mail_failure: bool,
        mail_calls: usize,
        changes: Vec<String>,
    }
    impl Host for FakeRuntime {
        fn reload(&mut self) -> Result<()> {
            Ok(())
        }
        fn restart(&mut self, _: &str) -> Result<()> {
            self.restarts += 1;
            Ok(())
        }
        fn health(&mut self, check: &Check, recovery: bool) -> Result<()> {
            if self.fail_live && !recovery && check.url.contains(":3000/") {
                Err(error("candidate unhealthy"))
            } else {
                Ok(())
            }
        }
    }
    impl Runtime for FakeRuntime {
        fn execute(
            &mut self,
            args: &[String],
            cwd: Option<&Path>,
            _: Option<&BTreeMap<String, String>>,
            _: Option<&[u8]>,
            _: u64,
        ) -> Result<String> {
            self.commands.push(args.to_vec());
            if args.get(1).is_some_and(|a| a == "rev-parse") {
                return Ok("a".repeat(40));
            }
            if args.iter().any(|a| a == "worktree") && args.iter().any(|a| a == "add") {
                let destination = PathBuf::from(&args[args.len() - 2]);
                fs::create_dir_all(destination.join("node_modules/next/dist/bin")).unwrap();
                fs::create_dir_all(destination.join("careers")).unwrap();
                fs::create_dir_all(destination.join("developer")).unwrap();
                fs::write(
                    destination.join("careers/server.py"),
                    "# synthetic managed service fixture",
                )
                .unwrap();
                fs::create_dir_all(destination.join(".next-production/static/chunks")).unwrap();
                fs::write(
                    destination.join(".next-production/static/chunks/current.js"),
                    "current",
                )
                .unwrap();
            }
            let _ = cwd;
            Ok(String::new())
        }
        fn api(&mut self, route: &str, method: &str, body: Option<&Value>) -> Result<Value> {
            if method == "POST" {
                self.api_posts.push(body.cloned().unwrap_or(Value::Null));
                return Ok(json!({"id":77}));
            }
            if route.contains("git/ref/") {
                return Ok(json!({"object":{"sha":"a".repeat(40)}}));
            }
            if route.contains("actions/workflows/") {
                let repo = if route.contains("CBGPU-backend") {
                    "CBGPU-Engineering/CBGPU-backend"
                } else {
                    "CBGPU-Engineering/CBGPU-web"
                };
                return Ok(
                    json!({"workflow_runs":[{"head_sha":"a".repeat(40),"head_branch":"main","event":"push","status":"completed","conclusion":"success","repository":{"full_name":repo},"html_url":format!("https://github.com/{repo}/actions/runs/1")}]}),
                );
            }
            if route.contains("/compare/") {
                return Ok(
                    json!({"status":"ahead","files":self.changes.iter().map(|p|json!({"filename":p})).collect::<Vec<_>>()}),
                );
            }
            Err(error("Unexpected fake API call"))
        }
        fn build(
            &mut self,
            args: &[String],
            _: &Path,
            env: &BTreeMap<String, String>,
            _: &Path,
        ) -> Result<()> {
            if args.iter().any(|a| a == "cbgpu-careers") {
                let path = PathBuf::from(&env["CARGO_TARGET_DIR"]).join("release/cbgpu-careers");
                fs::create_dir_all(path.parent().unwrap()).unwrap();
                fs::write(path, "synthetic executable").unwrap();
            }
            self.builds += 1;
            if self.fail_build {
                Err(error("isolated failed build"))
            } else {
                Ok(())
            }
        }
        fn preview(
            &mut self,
            args: &[String],
            env: &BTreeMap<String, String>,
            checks: &[Check],
        ) -> Result<()> {
            if args.first().is_some_and(|a| a.contains("cbgpu-careers")) {
                assert_eq!(env["CAREERS_ENABLE_OUTBOX"], "0");
                assert!(!env.contains_key("CAREERS_MAIL_ENV"));
                assert!(env["CAREERS_DATABASE_URL"].contains("default_transaction_read_only%3Don"));
            }
            assert!(checks.iter().all(|c| c.url.contains("127.0.0.1")));
            if self.fail_preview {
                Err(error("preview failed"))
            } else {
                Ok(())
            }
        }
        fn mail(&mut self, _: &str, _: &str, _: &Value) -> Result<()> {
            self.mail_calls += 1;
            if self.mail_failure {
                Err(error("provider offline"))
            } else {
                Ok(())
            }
        }
    }
    fn fixture(component: &str) -> (TempDir, Agent<FakeRuntime>) {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("delivery");
        let settings = tmp.path().join("units");
        let runtime = tmp.path().join("runtime");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&settings).unwrap();
        fs::create_dir_all(runtime.join("private")).unwrap();
        fs::write(runtime.join(".env"), "SMTP_PASS=\n").unwrap();
        let env = tmp.path().join("frontend.env");
        fs::write(
            &env,
            "NEXT_PUBLIC_TEST=visible\nEDUSET_TOKEN_SECRET=synthetic-secret\nSPOTIFY_CLIENT_ID=synthetic-client\nSPOTIFY_CLIENT_SECRET=synthetic-server-secret\nSPOTIFY_REDIRECT_URI=https://cbgpu.com/api/wakeup/spotify/callback\nCBGPU_AD_REVENUE_ENABLED=0\nCBGPU_AD_REVENUE_API_URL=http://127.0.0.1:3549\nNEXT_PUBLIC_CBGPU_AD_REVENUE_V1=0\nCBGPU_MULTISCHOOL_V1=0\nCBGPU_SCHOOLS_API_URL=http://127.0.0.1:3546\nCBGPU_TIMETABLE_WARM_SECRET_FILE=/private/timetable-warm-secret\nUNRELATED_PRIVATE_KEY=must-not-persist\n",
        )
        .unwrap();
        fs::write(settings.join("cbgpu-frontend.service"),"[Service]\nWorkingDirectory=/previous\nExecStart=/previous/server\nEnvironmentFile=/previous/.env\n").unwrap();
        let careers_env = tmp.path().join("careers.env");
        fs::write(&careers_env,"CAREERS_DATABASE_URL=postgresql://fixture:fixture@127.0.0.1/fixture\nCAREERS_FORM_KEY_PATH=/synthetic/form-key\nCAREERS_ENABLE_OUTBOX=1\nCAREERS_MAIL_ENV=/synthetic/mail\n").unwrap();
        fs::write(settings.join("cbgpu-careers.service"),format!("[Service]\nWorkingDirectory=/previous\nExecStart=/home/cbgpu/.local/lib/cbgpu-rust/services/cbgpu-careers serve\nEnvironmentFile={}\n",careers_env.display())).unwrap();
        let config = Config {
            repositories: BTreeMap::from([(
                component.into(),
                format!("CBGPU-Engineering/CBGPU-{component}"),
            )]),
            frontend_env: env,
            legacy_web: tmp.path().join("legacy/web"),
            runtime_backend: runtime,
            legacy_repo: tmp.path().join("legacy"),
            native_ai_binary: None,
        };
        let state = json!({"components":{component:{"sha":"b".repeat(40),"release":"previous"}},"paused":false});
        (
            tmp,
            Agent {
                root,
                settings,
                config,
                state,
                runtime: FakeRuntime::default(),
            },
        )
    }
    #[test]
    fn native_once_web_checks_ci_build_preview_switch_and_reports_success() {
        let (_tmp, mut agent) = fixture("web");
        agent.state["components"]["web"]["poll_error"] = json!("old connection error");
        agent.state["components"]["web"]["waiting_for"] = json!("old CI run");
        agent.poll(Some("web")).unwrap();
        assert_eq!(agent.state["components"]["web"]["status"], "healthy");
        assert_eq!(agent.state["components"]["web"]["sha"], "a".repeat(40));
        assert!(agent.state["components"]["web"]["poll_error"].is_null());
        assert!(agent.state["components"]["web"]["waiting_for"].is_null());
        let events = fs::read_to_string(agent.root.join("events.jsonl")).unwrap();
        let event: Value = serde_json::from_str(events.lines().last().unwrap()).unwrap();
        assert_eq!(event["component"], "web");
        assert_eq!(event["commit"], "a".repeat(40));
        assert_eq!(agent.runtime.builds, 2);
        assert_eq!(agent.runtime.restarts, 1);
        assert!(
            agent
                .runtime
                .api_posts
                .iter()
                .any(|v| v["state"] == "success")
        );
        assert!(!agent.root.join("transaction.json").exists());
        let unit = fs::read_to_string(agent.settings.join("cbgpu-frontend.service")).unwrap();
        assert!(unit.contains(".env.production.local"));
        assert!(unit.contains(&"a".repeat(40)));
        let saved = fs::read_to_string(
            agent
                .root
                .join("releases/web")
                .join("a".repeat(40))
                .join("web/.env.production.local"),
        )
        .unwrap();
        assert!(saved.contains("SPOTIFY_CLIENT_ID=synthetic-client\n"));
        assert!(saved.contains("SPOTIFY_CLIENT_SECRET=synthetic-server-secret\n"));
        assert!(
            saved.contains("SPOTIFY_REDIRECT_URI=https://cbgpu.com/api/wakeup/spotify/callback\n")
        );
        assert!(!saved.contains("NEXT_PUBLIC_SPOTIFY"));
        assert!(!saved.contains("UNRELATED_PRIVATE_KEY"));
        assert!(saved.contains("CBGPU_AD_REVENUE_ENABLED=0\n"));
        assert!(saved.contains("CBGPU_AD_REVENUE_API_URL=http://127.0.0.1:3549\n"));
        assert!(saved.contains("NEXT_PUBLIC_CBGPU_AD_REVENUE_V1=0\n"));
        assert!(saved.contains("CBGPU_MULTISCHOOL_V1=0\n"));
        assert!(saved.contains("CBGPU_SCHOOLS_API_URL=http://127.0.0.1:3546\n"));
        assert!(
            saved.contains("CBGPU_TIMETABLE_WARM_SECRET_FILE=/private/timetable-warm-secret\n")
        );
        assert!(!saved.contains("NEXT_PUBLIC_CBGPU_TIMETABLE_WARM"));
    }
    #[test]
    fn backend_component_rollout_keeps_unrelated_units_and_previews_without_mail() {
        let (_tmp, mut agent) = fixture("backend");
        agent.runtime.changes = vec!["developer/server.mts".into()];
        let unit = "[Service]\nWorkingDirectory=/previous\nExecStart=/usr/bin/node /previous/developer/server.mts\n";
        fs::write(agent.settings.join("cbgpu-developer-portal.service"), unit).unwrap();
        fs::write(
            agent.settings.join("cbgpu-backup.service"),
            "native backup remains",
        )
        .unwrap();
        agent.poll(Some("backend")).unwrap();
        assert_eq!(agent.state["components"]["backend"]["status"], "healthy");
        assert_eq!(agent.runtime.restarts, 1);
        assert_eq!(
            fs::read_to_string(agent.settings.join("cbgpu-backup.service")).unwrap(),
            "native backup remains"
        );
    }

    #[test]
    fn careers_changes_build_and_activate_native_only_after_readonly_nonmail_preview() {
        let (_tmp, mut agent) = fixture("backend");
        agent.runtime.changes = vec!["careers/roles.json".into()];
        agent.poll(Some("backend")).unwrap();
        assert_eq!(agent.state["components"]["backend"]["status"], "healthy");
        let unit = fs::read_to_string(agent.settings.join("cbgpu-careers.service")).unwrap();
        assert!(unit.contains(".delivery-bin/cbgpu-careers serve"));
        assert!(!unit.contains("python"));
        let release = PathBuf::from(
            agent.state["components"]["backend"]["release"]
                .as_str()
                .unwrap(),
        );
        let env = fs::read_to_string(release.join(".careers.production.env")).unwrap();
        assert!(env.contains("CAREERS_FORM_KEY_PATH=/synthetic/form-key"));
        assert!(env.contains("CAREERS_ENABLE_OUTBOX=1"));
        assert!(!env.contains("default_transaction_read_only"));
    }

    #[test]
    fn failed_build_or_preview_never_switches_and_requires_explicit_retry() {
        for preview in [false, true] {
            let (_tmp, mut agent) = fixture("web");
            agent.runtime.fail_preview = preview;
            agent.runtime.fail_build = !preview;
            agent.poll(Some("web")).unwrap();
            assert_eq!(agent.runtime.restarts, 0);
            assert_eq!(
                agent.state["components"]["web"]["failed_sha"],
                "a".repeat(40)
            );
            let builds = agent.runtime.builds;
            agent.poll(Some("web")).unwrap();
            assert_eq!(agent.runtime.builds, builds);
            assert_eq!(agent.state["components"]["web"]["sha"], "b".repeat(40));
            agent.retry("web").unwrap();
            assert!(agent.state["components"]["web"]["failed_sha"].is_null());
        }
    }
    #[test]
    fn failed_live_health_restores_units_and_preserves_previous_sha() {
        let (_tmp, mut agent) = fixture("web");
        let before = fs::read_to_string(agent.settings.join("cbgpu-frontend.service")).unwrap();
        agent.runtime.fail_live = true;
        agent.poll(Some("web")).unwrap();
        assert_eq!(agent.runtime.restarts, 2);
        assert_eq!(
            fs::read_to_string(agent.settings.join("cbgpu-frontend.service")).unwrap(),
            before
        );
        assert_eq!(agent.state["components"]["web"]["sha"], "b".repeat(40));
        assert_eq!(agent.state["components"]["web"]["status"], "failed");
    }
    #[test]
    fn schema_change_is_deferred_before_checkout_build_or_restart() {
        let (_tmp, mut agent) = fixture("backend");
        agent.runtime.changes = vec!["supabase/migrations/candidate.sql".into()];
        agent.poll(Some("backend")).unwrap();
        assert_eq!(agent.runtime.builds, 0);
        assert_eq!(agent.runtime.restarts, 0);
        assert!(agent.runtime.commands.is_empty());
        assert!(
            agent.state["components"]["backend"]["waiting_for"]
                .as_str()
                .unwrap()
                .contains("Infrastructure")
        );
    }
    #[test]
    fn active_build_detection_and_consistent_sqlite_backup_are_native() {
        let tmp = tempfile::tempdir().unwrap();
        let runtime = tmp.path().join("runtime");
        let path = runtime.join("private/frontend-deployments/deploy.sqlite");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let db = Connection::open(&path).unwrap();
        db.execute_batch(
            "create table deployments(status text);insert into deployments values('building');",
        )
        .unwrap();
        assert!(busy_backend(&runtime).unwrap());
        db.execute("update deployments set status='ready'", [])
            .unwrap();
        assert!(!busy_backend(&runtime).unwrap());
        let root = tmp.path().join("delivery");
        backup_sqlite(&root, &runtime, &"a".repeat(40)).unwrap();
        let snapshot = Connection::open(
            root.join("snapshots")
                .join("a".repeat(40))
                .join("frontend-deployments/deploy.sqlite"),
        )
        .unwrap();
        assert_eq!(
            snapshot
                .query_row("select status from deployments", [], |r| r
                    .get::<_, String>(0))
                .unwrap(),
            "ready"
        );
    }
    #[test]
    fn outbox_retries_without_losing_unacknowledged_notifications() {
        let (_tmp, mut agent) = fixture("web");
        fs::write(
            agent.config.runtime_backend.join(".env"),
            "SMTP_PASS=re_synthetic_only\n",
        )
        .unwrap();
        agent
            .notify("web", &"a".repeat(40), "success", "fixture-id")
            .unwrap();
        agent.runtime.mail_failure = true;
        agent.flush_notifications().unwrap();
        assert!(agent.root.join("outbox/fixture-id.json").exists());
        agent.runtime.mail_failure = false;
        agent.flush_notifications().unwrap();
        assert!(!agent.root.join("outbox/fixture-id.json").exists());
        assert_eq!(agent.runtime.mail_calls, 2);
    }
    #[test]
    fn real_build_log_redacts_secret_on_failure() {
        let tmp = tempfile::tempdir().unwrap();
        let log = tmp.path().join("build.log");
        let env = BTreeMap::from([("TOKEN".into(), "synthetic-secret-value".into())]);
        assert!(
            SystemRuntime
                .build(
                    &argv(&["/bin/sh", "-c", "echo synthetic-secret-value; exit 1"]),
                    tmp.path(),
                    &env,
                    &log
                )
                .is_err()
        );
        let log = fs::read_to_string(log).unwrap();
        assert!(log.contains("[REDACTED]"));
        assert!(!log.contains("synthetic-secret-value"));
    }
}

#[cfg(test)]
mod preservation_tests {
    use super::*;
    #[test]
    fn migrated_careers_and_backup_units_are_preserved_before_source_mutation() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("delivery");
        let settings = tmp.path().join("units");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&settings).unwrap();
        let careers =
            "[Service]\nExecStart=/home/cbgpu/.local/lib/cbgpu-rust/services/cbgpu-careers\n";
        let backup =
            "[Service]\nExecStart=/home/cbgpu/.local/lib/cbgpu-rust/services/cbgpu-ops backup\n";
        fs::write(settings.join("cbgpu-careers.service"), careers).unwrap();
        fs::write(settings.join("cbgpu-backup.service"), backup).unwrap();
        let config = Config {
            repositories: BTreeMap::new(),
            frontend_env: tmp.path().join("env"),
            legacy_web: tmp.path().join("web"),
            runtime_backend: tmp.path().join("runtime"),
            legacy_repo: tmp.path().join("legacy"),
            native_ai_binary: None,
        };
        struct GuardRuntime {
            ai_change: bool,
        }
        impl Host for GuardRuntime {
            fn reload(&mut self) -> Result<()> {
                panic!("must not reload")
            }
            fn restart(&mut self, _: &str) -> Result<()> {
                panic!("must not restart")
            }
            fn health(&mut self, _: &Check, _: bool) -> Result<()> {
                panic!("must not touch live health")
            }
        }
        impl Runtime for GuardRuntime {
            fn execute(
                &mut self,
                _: &[String],
                _: Option<&Path>,
                _: Option<&BTreeMap<String, String>>,
                _: Option<&[u8]>,
                _: u64,
            ) -> Result<String> {
                panic!("must not checkout or mutate source")
            }
            fn api(&mut self, _: &str, _: &str, _: Option<&Value>) -> Result<Value> {
                Ok(
                    json!({"status":"ahead","files":[{"filename":if self.ai_change {"ai/server.py"} else {"careers/roles.json"}}]}),
                )
            }
            fn build(
                &mut self,
                _: &[String],
                _: &Path,
                _: &BTreeMap<String, String>,
                _: &Path,
            ) -> Result<()> {
                panic!("must not build")
            }
            fn preview(
                &mut self,
                _: &[String],
                _: &BTreeMap<String, String>,
                _: &[Check],
            ) -> Result<()> {
                panic!("must not preview")
            }
            fn mail(&mut self, _: &str, _: &str, _: &Value) -> Result<()> {
                panic!("must not send mail")
            }
        }
        let mut agent = Agent {
            root,
            settings: settings.clone(),
            config,
            state: json!({"components":{}}),
            runtime: GuardRuntime { ai_change: false },
        };
        let result = agent.deploy_backend(
            &"a".repeat(40),
            "CBGPU-Engineering/CBGPU-backend",
            Some(&"b".repeat(40)),
            &tmp.path().join("log"),
            false,
        );
        assert!(result.unwrap_err().starts_with("DEFERRED:"));
        assert_eq!(
            fs::read_to_string(settings.join("cbgpu-careers.service")).unwrap(),
            careers
        );
        assert_eq!(
            fs::read_to_string(settings.join("cbgpu-backup.service")).unwrap(),
            backup
        );
        let ai = "[Service]\nExecStart=/immutable/old/ai/server.py\n";
        fs::write(settings.join("cbgpu-ai.service"), ai).unwrap();
        agent.runtime.ai_change = true;
        agent.config.native_ai_binary = Some(tmp.path().join("unreviewed-native-ai"));
        let result = agent.deploy_backend(
            &"c".repeat(40),
            "CBGPU-Engineering/CBGPU-backend",
            Some(&"b".repeat(40)),
            &tmp.path().join("ai.log"),
            true,
        );
        assert!(
            result
                .unwrap_err()
                .contains("separately verified fenced data handoff")
        );
        assert_eq!(
            fs::read_to_string(settings.join("cbgpu-ai.service")).unwrap(),
            ai
        );
    }
}
