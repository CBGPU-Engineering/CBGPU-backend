pub mod policy;
use policy::{Result, within};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    process::Stdio,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{
    io::{AsyncRead, AsyncReadExt},
    process::Command,
};
#[derive(Deserialize, Clone)]
pub struct Config {
    pub id: String,
    #[serde(default)]
    pub git_url: String,
    #[serde(default = "branch", deserialize_with = "nullable_branch")]
    pub git_branch: String,
    #[serde(default)]
    pub env_vars: BTreeMap<String, String>,
    pub public_url: String,
    pub host_port: u16,
    #[serde(default = "port")]
    pub container_port: u16,
    #[serde(default = "memory")]
    pub memory_limit: String,
    #[serde(default = "cpu")]
    pub cpu_limit: String,
    #[serde(default)]
    pub access_scope: String,
    #[serde(default)]
    pub private_host: Option<String>,
    #[serde(default)]
    pub school_host: Option<String>,
    #[serde(default)]
    pub platform_generation: Option<String>,
}
fn nullable_branch<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<String, D::Error> {
    Ok(Option::<String>::deserialize(deserializer)?
        .filter(|s| !s.is_empty())
        .unwrap_or_else(branch))
}
fn branch() -> String {
    "main".into()
}
fn port() -> u16 {
    8000
}
fn memory() -> String {
    "4g".into()
}
fn cpu() -> String {
    "3".into()
}
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Attempt {
    pub namespace: String,
    pub file: PathBuf,
    pub release: PathBuf,
    #[serde(default)]
    pub gateway: String,
    #[serde(default)]
    pub volumes: Vec<String>,
    #[serde(default)]
    pub services: Vec<String>,
    #[serde(default)]
    pub socket: PathBuf,
    #[serde(default = "storage_budget")]
    pub storage_gb: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platform_generation: Option<String>,
}
fn storage_budget() -> u64 {
    40
}
#[derive(Clone)]
pub struct Redactor {
    values: Vec<String>,
}
impl Redactor {
    pub fn new(values: &BTreeMap<String, String>) -> Self {
        let mut values = values
            .iter()
            .filter(|(k, v)| {
                v.len() > 3
                    && !matches!(
                        k.as_str(),
                        "CBGPU_ROOT_DIRECTORY"
                            | "CBGPU_GATEWAY_SERVICE"
                            | "CBGPU_STORAGE_GB"
                            | "CBGPU_COMPOSE_FILE"
                            | "CBGPU_DEPLOY_ENGINE"
                            | "CBGPU_RUNTIME_PRESET"
                    )
            })
            .map(|(_, v)| v.clone())
            .collect::<Vec<_>>();
        values.sort_by_key(|v| std::cmp::Reverse(v.len()));
        Self { values }
    }
    pub fn clean(&self, text: &str) -> String {
        let mut text = text.to_owned();
        for value in &self.values {
            text = text.replace(value, "[REDACTED]");
            let encoded = value
                .bytes()
                .map(|b| {
                    if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
                        char::from(b).to_string()
                    } else {
                        format!("%{b:02X}")
                    }
                })
                .collect::<String>();
            text = text.replace(&encoded, "[REDACTED]");
        }
        let jwt = regex::Regex::new(r"\beyJ[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+\b")
            .expect("fixed regex");
        let text = jwt.replace_all(&text, "[REDACTED]");
        regex::Regex::new(r"(https?://|postgres(?:ql)?://)([^\s/@]+):([^\s/@]+)@")
            .expect("fixed regex")
            .replace_all(&text, "${1}[REDACTED]@")
            .into_owned()
    }
    pub fn framed_lines(&self, text: &str) -> Vec<String> {
        self.clean(text)
            .lines()
            .map(|line| {
                format!(
                    "| {}",
                    line.chars()
                        .map(|c| if c.is_control() { ' ' } else { c })
                        .take(6000)
                        .collect::<String>()
                )
            })
            .collect()
    }
    pub fn log(&self, text: &str) {
        for line in self.framed_lines(text) {
            println!("{line}");
        }
    }
}
pub struct Output {
    pub success: bool,
    pub text: String,
    pub stderr: String,
}
async fn bounded(mut reader: impl AsyncRead + Unpin) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 8192];
    loop {
        let count = reader
            .read(&mut buffer)
            .await
            .map_err(|_| "Cannot read tool output")?;
        if count == 0 {
            break;
        }
        if bytes.len() + count > 4 * 1024 * 1024 {
            return Err("Tool output exceeded safe capture limit".into());
        }
        bytes.extend_from_slice(&buffer[..count]);
    }
    Ok(bytes)
}
pub async fn tool(
    program: &str,
    args: &[String],
    cwd: &Path,
    home: &Path,
    seconds: u64,
) -> Result<Output> {
    let mut command = Command::new(program);
    command
        .args(args)
        .current_dir(cwd)
        .env_clear()
        .env(
            "PATH",
            std::env::var_os("PATH").unwrap_or_else(|| "/usr/local/bin:/usr/bin:/bin".into()),
        )
        .env("HOME", home)
        .env("LANG", "C.UTF-8")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("DOCKER_CONFIG", home)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = command
        .spawn()
        .map_err(|_| "Cannot start required deployment tool")?;
    let out = child.stdout.take().ok_or("Cannot capture output")?;
    let err = child.stderr.take().ok_or("Cannot capture errors")?;
    let result = tokio::time::timeout(Duration::from_secs(seconds), async {
        tokio::try_join!(bounded(out), bounded(err), async {
            child
                .wait()
                .await
                .map_err(|_| "Cannot wait for tool".to_owned())
        })
    })
    .await;
    match result {
        Ok(Ok((out, err, status))) => Ok(Output {
            success: status.success(),
            text: String::from_utf8_lossy(&out).into_owned(),
            stderr: String::from_utf8_lossy(&err).into_owned(),
        }),
        _ => {
            let _ = child.kill().await;
            Err("Deployment tool timed out or exceeded its output limit".into())
        }
    }
}
async fn checked(
    program: &str,
    args: &[String],
    cwd: &Path,
    home: &Path,
    seconds: u64,
) -> Result<String> {
    let out = tool(program, args, cwd, home, seconds).await?;
    if !out.success {
        return Err("Deployment tool failed; inspect sanitized diagnostics".into());
    }
    Ok(out.text)
}
fn args(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| (*s).into()).collect()
}
fn private_write(path: &Path, text: &str) -> Result<()> {
    use std::io::Write;
    let mut options = std::fs::OpenOptions::new();
    options.create(true).truncate(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .map_err(|_| "Cannot write deployment configuration")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))
            .map_err(|_| "Cannot protect deployment configuration")?;
    }
    file.write_all(text.as_bytes())
        .map_err(|_| "Cannot write deployment configuration")?;
    file.sync_all()
        .map_err(|_| "Cannot persist deployment configuration")?;
    Ok(())
}
pub fn validate_config(cfg: &Config) -> Result<(u64, f64)> {
    if uuid::Uuid::parse_str(&cfg.id).is_err() || cfg.id.to_lowercase() != cfg.id {
        return Err("Invalid project identifier".into());
    }
    if cfg.host_port < 1024 || cfg.container_port == 0 {
        return Err("Invalid deployment port".into());
    }
    let memory = cfg.memory_limit.to_lowercase();
    let (number, unit) = memory
        .split_at_checked(memory.len().saturating_sub(1))
        .ok_or("Invalid memory limit")?;
    let size = number.parse::<f64>().map_err(|_| "Invalid memory limit")?
        * match unit {
            "g" => 1024f64.powi(3),
            "m" => 1024f64.powi(2),
            _ => return Err("Use memory limits in m or g".into()),
        };
    let cpu = cfg
        .cpu_limit
        .parse::<f64>()
        .map_err(|_| "Invalid CPU limit")?;
    if !size.is_finite()
        || !(128f64 * 1024. * 1024. ..=16f64 * 1024f64.powi(3)).contains(&size)
        || !cpu.is_finite()
        || !(0.1..=16.).contains(&cpu)
    {
        return Err("Requested resources exceed deployment limits".into());
    }
    if cfg.env_vars.len() > 1024
        || cfg.env_vars.iter().any(|(k, v)| {
            !regex::Regex::new(r"^[A-Za-z_][A-Za-z0-9_]*$")
                .expect("fixed regex")
                .is_match(k)
                || v.contains(['\n', '\r', '\0'])
        })
    {
        return Err("Invalid single-line environment setting".into());
    }
    if !["", "public", "private", "school"].contains(&cfg.access_scope.as_str()) {
        return Err("Invalid access scope".into());
    }
    Ok((size as u64, cpu))
}
/// Infrastructure-owned immutable generation labels; customer Compose cannot forge them.
pub fn apply_platform_generation(raw: &mut Value, cfg: &Config) -> Result<Option<String>> {
    let generation = cfg
        .platform_generation
        .as_ref()
        .map(|v| {
            uuid::Uuid::parse_str(v)
                .map(|v| v.to_string())
                .map_err(|_| "Invalid platform generation".to_string())
        })
        .transpose()?;
    let services = raw["services"]
        .as_object_mut()
        .ok_or("Invalid managed services")?;
    for service in services.values_mut() {
        if service.get("labels").is_none() || service["labels"].is_null() {
            service["labels"] = json!({})
        }
        let labels = service["labels"]
            .as_object_mut()
            .ok_or("Invalid service labels")?;
        labels.retain(|key, _| {
            !key.starts_with("cbgpu.platform.")
                && (generation.is_none() || !key.starts_with("com.docker.compose."))
        });
        if let Some(g) = &generation {
            labels.insert("cbgpu.platform.generation".into(), json!(g));
            labels.insert("cbgpu.platform.project".into(), json!(cfg.id));
        }
    }
    Ok(generation)
}
pub async fn prepare(
    cfg: &Config,
    base: &Path,
    fixture: Option<&Path>,
) -> Result<(Attempt, Redactor)> {
    let (memory, cpu) = validate_config(cfg)?;
    let id = uuid::Uuid::parse_str(&cfg.id)
        .map_err(|_| "Invalid project identifier")?
        .to_string();
    if id != cfg.id {
        return Err("Use a canonical project UUID".into());
    }
    let base = base
        .canonicalize()
        .map_err(|_| "Compose storage directory does not exist")?;
    let namespace = format!("cbgpu-compose-{id}");
    let release = base.join(&id).join(format!(
        "release-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| "Invalid system clock")?
            .as_nanos()
    ));
    std::fs::create_dir_all(&release).map_err(|_| "Cannot create deployment directory")?;
    let source = release.join("source");
    if let Some(fixture) = fixture {
        copy_fixture(fixture, &source)?;
    } else {
        if !regex::Regex::new(r"^https://github\.com/[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+/?$")
            .expect("fixed regex")
            .is_match(&cfg.git_url)
            || !regex::Regex::new(r"^[A-Za-z0-9][A-Za-z0-9_./-]{0,99}$")
                .expect("fixed regex")
                .is_match(&cfg.git_branch)
            || cfg.git_branch.contains("..")
        {
            return Err("Use a credential-free GitHub HTTPS repository and valid branch".into());
        }
        checked(
            "git",
            &args(&[
                "-c",
                "protocol.file.allow=never",
                "-c",
                "core.hooksPath=/dev/null",
                "clone",
                "--depth=1",
                "--single-branch",
                "--branch",
                &cfg.git_branch,
                "--",
                &cfg.git_url,
                source.to_str().ok_or("Invalid repository path")?,
            ]),
            &release,
            &release,
            120,
        )
        .await?;
    }
    let mut bytes = 0_u64;
    for (count, entry) in walkdir::WalkDir::new(&source)
        .follow_links(false)
        .into_iter()
        .enumerate()
    {
        let entry = entry.map_err(|_| "Cannot inspect repository")?;
        if count > 250000 {
            return Err("Repository contains too many files".into());
        }
        if entry.file_type().is_symlink() {
            within(
                &source,
                entry.path().to_str().ok_or("Invalid repository path")?,
            )?;
        } else if entry.file_type().is_file() {
            bytes = bytes.saturating_add(
                entry
                    .metadata()
                    .map_err(|_| "Cannot inspect repository")?
                    .len(),
            );
            if bytes > 2 * 1024 * 1024 * 1024 {
                return Err("Repository exceeds workspace budget".into());
            }
        }
    }
    let mut values = cfg.env_vars.clone();
    let root = within(
        &source,
        values
            .get("CBGPU_ROOT_DIRECTORY")
            .filter(|v| !v.is_empty())
            .map_or(".", String::as_str),
    )?;
    if !root.is_dir() {
        return Err("Root Directory does not exist".into());
    }
    let file = if let Some(name) = values.get("CBGPU_COMPOSE_FILE").filter(|v| !v.is_empty()) {
        within(&root, name)?
    } else {
        [
            "compose.yaml",
            "compose.yml",
            "docker-compose.yml",
            "docker-compose.yaml",
        ]
        .iter()
        .map(|name| root.join(name))
        .find(|p| p.is_file())
        .ok_or("Compose file not found in Root Directory")?
    };
    if std::fs::metadata(&file)
        .map_err(|_| "Compose file not found")?
        .len()
        > 1024 * 1024
    {
        return Err("Compose file is too large".into());
    }
    let dotenv = within(&root, ".env")?;
    if dotenv.is_file() {
        let text =
            std::fs::read_to_string(&dotenv).map_err(|_| "Cannot read project environment")?;
        if text.len() > 1024 * 1024 {
            return Err("Project environment is too large".into());
        }
        for line in text.lines().filter(|l| !l.trim_start().starts_with('#')) {
            if let Some((key, value)) = line.split_once('=') {
                values
                    .entry(key.trim().into())
                    .or_insert(value.trim().trim_matches('"').trim_matches('\'').into());
            }
        }
    }
    for key in [
        "CBGPU_PUBLIC_URL",
        "API_EXTERNAL_URL",
        "SUPABASE_PUBLIC_URL",
    ] {
        values.entry(key.into()).or_insert(cfg.public_url.clone());
    }
    if values.values().any(|v| v.contains(['\n', '\r', '\0'])) {
        return Err("Compose environment values must be single-line".into());
    }
    private_write(
        &dotenv,
        &values
            .iter()
            .map(|(k, v)| format!("{k}='{}'\n", v.replace('\'', "\\'")))
            .collect::<String>(),
    )?;
    let socket = base
        .parent()
        .ok_or("Invalid Compose storage")?
        .join("compose-sockets")
        .join(format!("{id}.sock"));
    std::fs::create_dir_all(socket.parent().ok_or("Invalid socket path")?)
        .map_err(|_| "Cannot create log socket directory")?;
    let mut raw =
        policy::parse(&std::fs::read_to_string(&file).map_err(|_| "Cannot read Compose file")?)?;
    policy::validate(&mut raw, &root, &values, &socket)?;
    raw["name"] = json!(namespace);
    for (name, service) in raw["services"].as_object_mut().ok_or("Invalid services")? {
        service["container_name"] = json!(format!("{namespace}-{name}"));
    }
    let intermediate = release.join(".cbgpu-compose-input.yml");
    private_write(
        &intermediate,
        &serde_yaml_ng::to_string(&raw).map_err(|_| "Cannot encode validated Compose file")?,
    )?;
    let text = checked(
        "docker",
        &args(&[
            "compose",
            "--project-directory",
            root.to_str().ok_or("Invalid Root Directory")?,
            "--env-file",
            dotenv.to_str().ok_or("Invalid environment path")?,
            "-p",
            &namespace,
            "-f",
            intermediate.to_str().ok_or("Invalid Compose path")?,
            "config",
            "--format",
            "json",
        ]),
        &root,
        &release,
        30,
    )
    .await?;
    let mut normalized: Value =
        serde_json::from_str(&text).map_err(|_| "Invalid normalized Compose result")?;
    let services = normalized["services"]
        .as_object()
        .ok_or("Invalid normalized services")?;
    let gateway = values
        .get("CBGPU_GATEWAY_SERVICE")
        .filter(|v| !v.is_empty())
        .cloned()
        .or_else(|| {
            ["api-gw", "kong", "web", "app"]
                .iter()
                .find(|v| services.contains_key(**v))
                .map(|v| (*v).to_owned())
        })
        .or_else(|| services.keys().next().cloned())
        .ok_or("No Compose services")?;
    let extra = match cfg.access_scope.as_str() {
        "private" => Some(cfg.private_host.as_deref().unwrap_or("100.100.136.37")),
        "school" => Some(cfg.school_host.as_deref().unwrap_or("10.138.23.12")),
        _ => None,
    };
    if extra.is_some_and(|ip| ip.parse::<std::net::IpAddr>().is_err()) {
        return Err("Invalid deployment bind address".into());
    }
    let volumes = policy::normalize(
        &mut normalized,
        &namespace,
        &source,
        &socket,
        &gateway,
        policy::Limits {
            memory,
            cpu,
            host_port: cfg.host_port,
            container_port: cfg.container_port,
            extra_address: extra,
        },
    )?;
    let platform_generation = apply_platform_generation(&mut normalized, cfg)?;
    let services = normalized["services"]
        .as_object()
        .ok_or("Invalid services")?
        .keys()
        .cloned()
        .collect();
    let managed = release.join("compose-managed.json");
    private_write(
        &managed,
        &serde_json::to_string(&normalized).map_err(|_| "Cannot encode managed Compose file")?,
    )?;
    let storage_gb = values
        .get("CBGPU_STORAGE_GB")
        .map_or(Ok(40), |v| v.parse::<u64>())
        .map_err(|_| "Invalid storage budget")?;
    if !(40..=200).contains(&storage_gb) {
        return Err("Invalid storage budget".into());
    }
    let mut redactor = Redactor::new(&values);
    let sensitive = regex::Regex::new("(?i)SECRET|TOKEN|PASSWORD|PASSWD|KEY|DATABASE_URL|DB_URL")
        .expect("fixed regex");
    for service in normalized["services"]
        .as_object()
        .ok_or("Invalid services")?
        .values()
    {
        if let Some(environment) = service["environment"].as_object() {
            for (k, v) in environment {
                if sensitive.is_match(k)
                    && let Some(value) = v.as_str().filter(|v| v.len() > 3)
                {
                    redactor.values.push(value.into());
                }
            }
        }
    }
    redactor.values.sort_by_key(|v| std::cmp::Reverse(v.len()));
    Ok((
        Attempt {
            platform_generation,
            namespace,
            file: managed,
            release,
            gateway,
            volumes,
            services,
            socket,
            storage_gb,
        },
        redactor,
    ))
}
fn copy_fixture(from: &Path, to: &Path) -> Result<()> {
    std::fs::create_dir_all(to).map_err(|_| "Cannot create fixture directory")?;
    for entry in walkdir::WalkDir::new(from).follow_links(false) {
        let entry = entry.map_err(|_| "Cannot read fixture")?;
        let dest = to.join(
            entry
                .path()
                .strip_prefix(from)
                .map_err(|_| "Invalid fixture")?,
        );
        if entry.file_type().is_dir() {
            std::fs::create_dir_all(dest).map_err(|_| "Cannot copy fixture")?;
        } else if entry.file_type().is_file() {
            std::fs::copy(entry.path(), dest).map_err(|_| "Cannot copy fixture")?;
        } else {
            return Err("Symlinks are not supported in fixture copies".into());
        }
    }
    Ok(())
}
pub async fn diagnostics(attempt: &Attempt, redactor: &Redactor) -> Result<Value> {
    let start = std::time::Instant::now();
    let mut compose = args(&[
        "compose",
        "-p",
        &attempt.namespace,
        "-f",
        attempt.file.to_str().ok_or("Invalid Compose path")?,
    ]);
    async fn capture(
        args: Vec<String>,
        attempt: &Attempt,
        redactor: &Redactor,
        start: std::time::Instant,
        limit: usize,
    ) -> String {
        let remaining = 90_u64.saturating_sub(start.elapsed().as_secs());
        if remaining == 0 {
            return "[diagnostic collection deadline reached]".into();
        }
        match tool(
            "docker",
            &args,
            &attempt.release,
            &attempt.release,
            remaining.min(30),
        )
        .await
        {
            Ok(out) => {
                let text = redactor.clean(&(out.text + &out.stderr));
                text.chars()
                    .rev()
                    .take(limit)
                    .collect::<String>()
                    .chars()
                    .rev()
                    .collect()
            }
            Err(_) => "[diagnostic capture unavailable]".into(),
        }
    }
    let mut command = compose.clone();
    command.extend(args(&["ps", "-a"]));
    let ps = capture(command, attempt, redactor, start, 20000).await;
    let mut command = compose.clone();
    command.extend(args(&["logs", "--no-color", "--timestamps", "--tail=200"]));
    let logs = capture(command, attempt, redactor, start, 100000).await;
    let ids = capture(
        args(&[
            "ps",
            "-a",
            "--filter",
            &format!("label=com.docker.compose.project={}", attempt.namespace),
            "--format",
            "{{.ID}}",
        ]),
        attempt,
        redactor,
        start,
        2000,
    )
    .await;
    let mut services = Vec::new();
    let mut failed = Vec::<String>::new();
    let valid = regex::Regex::new(r"^[a-f0-9]{12,64}$").expect("fixed regex");
    for id in ids.lines().filter(|s| valid.is_match(s)).take(20) {
        let raw = capture(
            args(&["inspect", "--format", "{{json .State}}", id]),
            attempt,
            redactor,
            start,
            18000,
        )
        .await;
        let name = capture(
            args(&[
                "inspect",
                "--format",
                "{{index .Config.Labels \"com.docker.compose.service\"}}",
                id,
            ]),
            attempt,
            redactor,
            start,
            200,
        )
        .await
        .trim()
        .to_owned();
        let state: Value = serde_json::from_str(&raw).unwrap_or(Value::Null);
        let status = state["Status"].as_str().unwrap_or("unknown");
        let health = state["Health"]["Status"].clone();
        let bad = health == "unhealthy"
            || health == "starting"
            || status != "running" && !(status == "exited" && state["ExitCode"] == 0);
        let mut entry = json!({"service":name,"container":id,"status":status,"health":health,"exit_code":state["ExitCode"],"oom_killed":state["OOMKilled"].as_bool().unwrap_or(false),"error":state["Error"].as_str().unwrap_or("")});
        if bad {
            failed.push(if name.is_empty() { id.into() } else { name });
            entry["health_output"] = json!(
                capture(
                    args(&["inspect", "--format", "{{json .State.Health.Log}}", id]),
                    attempt,
                    redactor,
                    start,
                    16000
                )
                .await
            );
        }
        services.push(entry);
    }
    for name in &attempt.services {
        if !services.iter().any(|s| s["service"] == *name) {
            failed.push(name.clone());
            services.push(json!({"service":name,"container":"not-created","status":"missing","health":null,"exit_code":null,"oom_killed":false,"error":"Container was not created or could not be inspected","health_output":"No health-check history available."}));
        }
    }
    let value = json!({"failed_services":failed,"services":services,"compose_ps":ps,"container_logs":logs,"captured_at":chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs,true),"containers_preserved":true,"volumes_preserved":true});
    private_write(
        &attempt.release.join("failure-diagnostics.json"),
        &serde_json::to_string(&value).map_err(|_| "Cannot encode diagnostics")?,
    )?;
    redactor.log(&format!(
        "Compose service status:\n{ps}\nContainer logs:\n{logs}"
    ));
    println!(
        "CBGPU_DIAGNOSTICS {}",
        serde_json::to_string(&value).map_err(|_| "Cannot encode diagnostics")?
    );
    compose.clear();
    Ok(value)
}
pub async fn deploy(cfg: &Config, base: &Path, fixture: Option<&Path>) -> Result<Attempt> {
    let (attempt, redactor) = prepare(cfg, base, fixture).await?;
    println!(
        "CBGPU_ATTEMPT {}",
        serde_json::to_string(&attempt).map_err(|_| "Cannot encode attempt")?
    );
    let managed: Value = serde_json::from_str(
        &std::fs::read_to_string(&attempt.file).map_err(|_| "Cannot read managed Compose file")?,
    )
    .map_err(|_| "Invalid managed Compose file")?;
    if managed["services"]
        .as_object()
        .ok_or("Missing services")?
        .values()
        .any(|s| !s["build"].is_null())
    {
        let client = base
            .parent()
            .ok_or("Invalid Compose base")?
            .join("compose-docker-client");
        std::fs::create_dir_all(&client).map_err(|_| "Cannot create build client directory")?;
        let builder = format!("cbgpu-compose-build-{}", &cfg.id[..12]);
        let inspect = tool(
            "docker",
            &args(&["buildx", "inspect", &builder]),
            &attempt.release,
            &client,
            30,
        )
        .await?;
        if !inspect.success {
            checked(
                "docker",
                &args(&[
                    "buildx",
                    "create",
                    "--name",
                    &builder,
                    "--driver",
                    "docker-container",
                    "--driver-opt",
                    "memory=2g,memory-swap=2g,cpu-quota=200000,cpu-period=100000,default-load=true",
                ]),
                &attempt.release,
                &client,
                30,
            )
            .await?;
        }
        let built = tool(
            "docker",
            &args(&[
                "compose",
                "-p",
                &attempt.namespace,
                "-f",
                attempt.file.to_str().ok_or("Invalid Compose path")?,
                "build",
                "--builder",
                &builder,
            ]),
            &attempt.release,
            &client,
            900,
        )
        .await;
        let _ = tool(
            "docker",
            &args(&["buildx", "stop", &builder]),
            &attempt.release,
            &client,
            30,
        )
        .await;
        match built {
            Ok(out) => {
                redactor.log(&(out.text + &out.stderr));
                if !out.success {
                    return Err(
                        "Compose image build failed; containers and volumes were retained".into(),
                    );
                }
            }
            Err(e) => return Err(e),
        }
    }
    let up = tool(
        "docker",
        &args(&[
            "compose",
            "-p",
            &attempt.namespace,
            "-f",
            attempt.file.to_str().ok_or("Invalid Compose path")?,
            "up",
            "-d",
            "--no-build",
            "--wait",
            "--wait-timeout",
            "300",
            "--remove-orphans",
        ]),
        &attempt.release,
        &attempt.release,
        900,
    )
    .await;
    match up {
        Ok(out) if out.success => {
            redactor.log(&(out.text + &out.stderr));
            Ok(attempt)
        }
        failed => {
            if let Ok(out) = failed {
                redactor.log(&(out.text + &out.stderr));
            }
            let _ = diagnostics(&attempt, &redactor).await;
            Err("Compose deployment failed. Containers and persistent volumes were retained for diagnosis.".into())
        }
    }
}

pub async fn measure(attempt: &Attempt, base: &Path) -> Result<u64> {
    let id = attempt
        .namespace
        .strip_prefix("cbgpu-compose-")
        .ok_or("Invalid project namespace")?;
    if uuid::Uuid::parse_str(id)
        .map_err(|_| "Invalid project namespace")?
        .to_string()
        != id
    {
        return Err("Invalid project namespace".into());
    }
    let valid = regex::Regex::new(r"^[a-zA-Z0-9][a-zA-Z0-9_.-]{0,254}$").expect("fixed regex");
    let prefix = format!("{}-", attempt.namespace);
    if attempt.volumes.len() > 256
        || attempt
            .volumes
            .iter()
            .any(|v| !v.starts_with(&prefix) || !valid.is_match(v))
    {
        return Err("Invalid project volume".into());
    }
    let mut size = 0_u64;
    if !attempt.volumes.is_empty() {
        let mut command = args(&[
            "run",
            "--rm",
            "--network",
            "none",
            "--read-only",
            "--memory",
            "64m",
            "--cpus",
            "0.25",
            "--pids-limit",
            "32",
            "--entrypoint",
            "/bin/sh",
        ]);
        for (i, name) in attempt.volumes.iter().enumerate() {
            command.extend(args(&[
                "--mount",
                &format!("type=volume,src={name},dst=/v{i},readonly"),
            ]));
        }
        let targets = (0..attempt.volumes.len())
            .map(|i| format!("/v{i}"))
            .collect::<Vec<_>>()
            .join(" ");
        command.extend(args(&["nginx:alpine", "-c", &format!("du -sk {targets}")]));
        let text = checked("docker", &command, base, base, 90).await?;
        for line in text.lines() {
            let kib = line
                .split_whitespace()
                .next()
                .ok_or("Invalid volume size")?
                .parse::<u64>()
                .map_err(|_| "Invalid volume size")?;
            size = size
                .checked_add(kib.checked_mul(1024).ok_or("Storage size overflow")?)
                .ok_or("Storage size overflow")?;
        }
    }
    let ids = checked(
        "docker",
        &args(&[
            "ps",
            "-a",
            "--filter",
            &format!("label=com.docker.compose.project={}", attempt.namespace),
            "--format",
            "{{.ID}}",
        ]),
        base,
        base,
        10,
    )
    .await?;
    let valid_id = regex::Regex::new(r"^[a-f0-9]{12,64}$").expect("fixed regex");
    let ids = ids.split_whitespace().collect::<Vec<_>>();
    if ids.len() > 20 || ids.iter().any(|id| !valid_id.is_match(id)) {
        return Err("Invalid project container list".into());
    }
    if !ids.is_empty() {
        let mut command = args(&["inspect", "--size", "--format", "{{.SizeRw}}"]);
        command.extend(ids.into_iter().map(str::to_owned));
        for line in checked("docker", &command, base, base, 90).await?.lines() {
            size = size
                .checked_add(
                    line.trim()
                        .parse::<u64>()
                        .map_err(|_| "Invalid container size")?,
                )
                .ok_or("Storage size overflow")?;
        }
    }
    Ok(size)
}

pub async fn capture_existing(id: &str, base: &Path) -> Result<()> {
    use sqlx::{
        Row,
        sqlite::{SqliteConnectOptions, SqlitePoolOptions},
    };
    if uuid::Uuid::parse_str(id)
        .map_err(|_| "Invalid project identifier")?
        .to_string()
        != id
    {
        return Err("Invalid project identifier".into());
    }
    let database = base
        .parent()
        .ok_or("Invalid Compose base")?
        .join("frontend-deployments/deploy.sqlite");
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(
            SqliteConnectOptions::new()
                .filename(database)
                .create_if_missing(false),
        )
        .await
        .map_err(|_| "Cannot open existing deployment state")?;
    let row =
        sqlx::query("SELECT attempt_result,result,logs FROM compose_states WHERE project_id=? AND status IN ('error','interrupted')")
            .bind(id)
            .fetch_optional(&pool)
            .await
            .map_err(|_| "Cannot read deployment state")?
            .ok_or("No Compose deployment state")?;
    let raw = row
        .try_get::<Option<String>, _>("attempt_result")
        .map_err(|_| "Invalid deployment state")?
        .or(row
            .try_get::<Option<String>, _>("result")
            .map_err(|_| "Invalid deployment state")?)
        .ok_or("No stored deployment attempt")?;
    let attempt: Attempt =
        serde_json::from_str(&raw).map_err(|_| "Invalid stored deployment attempt")?;
    let project = base
        .join(id)
        .canonicalize()
        .map_err(|_| "Missing project storage")?;
    let release = attempt
        .release
        .canonicalize()
        .map_err(|_| "Missing deployment release")?;
    let file = attempt
        .file
        .canonicalize()
        .map_err(|_| "Missing deployment configuration")?;
    if attempt.namespace != format!("cbgpu-compose-{id}")
        || !release.starts_with(&project)
        || file != release.join("compose-managed.json")
    {
        return Err("Stored deployment is outside its project namespace".into());
    }
    let managed: Value = serde_json::from_str(
        &std::fs::read_to_string(&attempt.file).map_err(|_| "Cannot read managed configuration")?,
    )
    .map_err(|_| "Invalid managed configuration")?;
    let mut values = BTreeMap::new();
    if let Some(services) = managed["services"].as_object() {
        for (name, service) in services {
            if let Some(environment) = service["environment"].as_object() {
                for (k, v) in environment {
                    if let Some(value) = v.as_str() {
                        values.insert(format!("{name}_{k}"), value.into());
                    }
                }
            }
        }
    }
    let redactor = Redactor::new(&values);
    let diag = diagnostics(&attempt, &redactor).await?;
    let logs = row
        .try_get::<Option<String>, _>("logs")
        .map_err(|_| "Invalid deployment logs")?
        .unwrap_or_default();
    let logs = format!(
        "{}\n{}\n{}",
        redactor
            .clean(&logs)
            .chars()
            .rev()
            .take(25000)
            .collect::<String>()
            .chars()
            .rev()
            .collect::<String>(),
        diag["compose_ps"].as_str().unwrap_or(""),
        diag["container_logs"].as_str().unwrap_or("")
    );
    let message = format!(
        "Compose deployment failed: {}. Containers and persistent volumes were retained.",
        diag["failed_services"]
            .as_array()
            .ok_or("Invalid diagnostics")?
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join(", ")
    );
    sqlx::query("UPDATE compose_states SET diagnostics=?,attempt_result=?,logs=?,error=?,updated_at=? WHERE project_id=?").bind(serde_json::to_string(&diag).map_err(|_|"Cannot encode diagnostics")?).bind(raw).bind(logs.chars().rev().take(200000).collect::<String>().chars().rev().collect::<String>()).bind(message).bind(chrono::Utc::now().to_rfc3339()).bind(id).execute(&pool).await.map_err(|_|"Cannot save deployment diagnostics")?;
    pool.close().await;
    Ok(())
}
