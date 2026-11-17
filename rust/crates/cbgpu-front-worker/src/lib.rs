use base64::{Engine, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt},
    process::Command,
};
use walkdir::WalkDir;

pub const WORKSPACE_LIMIT: u64 = 2 * 1024 * 1024 * 1024;
pub const FILE_LIMIT: usize = 250_000;
pub const ARTIFACT_LIMIT: u64 = 250 * 1024 * 1024;
#[derive(Deserialize)]
pub struct Config {
    pub directory: PathBuf,
    pub deployment_id: String,
    pub repository: String,
    pub branch: String,
    pub framework: String,
    #[serde(default)]
    pub root_directory: String,
    #[serde(default)]
    pub output_directory: String,
    #[serde(default)]
    pub install_command: String,
    #[serde(default)]
    pub build_command: String,
    #[serde(default)]
    pub git_token: String,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}
#[derive(Serialize, Debug)]
pub struct Deployment {
    pub kind: String,
    pub directory: PathBuf,
    pub commit: String,
    pub framework: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub container: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub public: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spa: Option<bool>,
}
#[derive(Clone)]
pub struct Redactor {
    secrets: Arc<Vec<String>>,
}
impl Redactor {
    pub fn new(config: &Config) -> Self {
        let mut secrets = Vec::new();
        for value in std::iter::once(&config.git_token).chain(config.env.values()) {
            if value.chars().count() >= 4 {
                secrets.push(value.clone());
                let quoted = serde_json::to_string(value).expect("serializable string");
                secrets.push(quoted[1..quoted.len() - 1].to_owned());
                for line in value.lines() {
                    if line.chars().count() >= 4 {
                        secrets.push(line.into());
                    }
                }
            }
        }
        if !config.git_token.is_empty() {
            secrets.push(STANDARD.encode(format!("x-access-token:{}", config.git_token)));
        }
        secrets.sort_by_key(|s| std::cmp::Reverse(s.len()));
        secrets.dedup();
        Self {
            secrets: Arc::new(secrets),
        }
    }
    pub fn redact(&self, value: &str) -> String {
        let mut value = value.to_owned();
        for secret in self.secrets.iter() {
            value = value.replace(secret, "[REDACTED]");
        }
        value
    }
    pub fn log(&self, value: &str) {
        for line in self.redact(value).split('\n') {
            println!("{}", framed_log_line(line));
        }
    }
}
/// Worker control records use stdout too. Never let untrusted build output
/// create another record through a carriage return or other control character.
fn framed_log_line(value: &str) -> String {
    let mut framed = String::from("| ");
    for character in value.chars().take(6000) {
        match character {
            '\r' => framed.push_str("\\r"),
            '\n' => framed.push_str("\\n"),
            character if character.is_control() && character != '\t' => {
                use std::fmt::Write;
                write!(framed, "\\u{{{:x}}}", character as u32).expect("String formatting");
            }
            character => framed.push(character),
        }
    }
    framed
}
pub fn validate(config: &Config) -> Result<(), String> {
    if uuid::Uuid::parse_str(&config.deployment_id).is_err() {
        return Err("Invalid deployment identifier".into());
    }
    if !regex::Regex::new(r"^https://github\.com/[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+/?$")
        .expect("fixed regex")
        .is_match(&config.repository)
    {
        return Err("Use a credential-free GitHub HTTPS repository URL".into());
    }
    if !regex::Regex::new(r"^[A-Za-z0-9][A-Za-z0-9_./-]{0,99}$")
        .expect("fixed regex")
        .is_match(&config.branch)
        || config.branch.contains("..")
    {
        return Err("Invalid Git branch".into());
    }
    if !["auto", "next", "vite", "astro", "static", "other"].contains(&config.framework.as_str()) {
        return Err("Unsupported frontend framework".into());
    }
    for value in [&config.root_directory, &config.output_directory] {
        if value.len() > 500
            || Path::new(value).is_absolute()
            || value.split(['/', '\\']).any(|part| part == "..")
            || value.contains(['\0', '\r', '\n', '\\'])
        {
            return Err("Build directories must stay inside the repository".into());
        }
    }
    for command in [&config.install_command, &config.build_command] {
        if command.len() > 500 || command.contains(['\0', '\r', '\n']) {
            return Err("Invalid build command".into());
        }
    }
    if config.git_token.len() > 1000 || config.git_token.contains(['\r', '\n', '\0']) {
        return Err("Invalid Git credential format".into());
    }
    if config.env.len() > 40 {
        return Err("Use up to 40 environment variables".into());
    }
    let key = regex::Regex::new(r"^[A-Za-z_][A-Za-z0-9_]{0,99}$").expect("fixed regex");
    for (name, value) in &config.env {
        if !key.is_match(name)
            || value.len() > 32768
            || value.chars().count() > 8192
            || value.contains('\0')
            || [
                "NODE_OPTIONS",
                "LD_PRELOAD",
                "PATH",
                "HOME",
                "GIT_CONFIG",
                "DOCKER_HOST",
            ]
            .iter()
            .any(|reserved| name == reserved || name.starts_with(&format!("{reserved}_")))
        {
            return Err("Reserved or invalid frontend environment variable".into());
        }
    }
    Ok(())
}
pub fn within(root: &Path, relative: &str) -> Result<PathBuf, String> {
    let canonical = root
        .canonicalize()
        .map_err(|_| "Repository directory unavailable")?;
    let value = root
        .join(if relative.is_empty() { "." } else { relative })
        .canonicalize()
        .map_err(|_| "Build/output directory does not exist")?;
    if !value.starts_with(&canonical) || !value.is_dir() {
        return Err("Directory must stay inside this repository".into());
    }
    Ok(value)
}
pub fn workspace_size(root: &Path) -> bool {
    let mut size = 0u64;
    let mut count = 0usize;
    for item in WalkDir::new(root).follow_links(false) {
        let Ok(item) = item else {
            return false;
        };
        if item.file_type().is_dir() {
            continue;
        }
        let Ok(meta) = std::fs::symlink_metadata(item.path()) else {
            return false;
        };
        size = size.saturating_add(meta.len());
        count += 1;
        if size > WORKSPACE_LIMIT || count > FILE_LIMIT {
            return false;
        }
    }
    true
}
pub fn detect_framework(config: &Config, package: &Value) -> String {
    if config.framework != "auto" {
        return config.framework.clone();
    }
    let dependency = |name: &str| {
        ["dependencies", "devDependencies"]
            .iter()
            .any(|kind| package.get(kind).and_then(|p| p.get(name)).is_some())
    };
    if dependency("next") {
        "next"
    } else if dependency("vite") {
        "vite"
    } else if dependency("astro") {
        "astro"
    } else if package.is_null() {
        "static"
    } else {
        "other"
    }
    .into()
}
pub fn publish(source: &Path, artifact: &Path) -> Result<(), String> {
    std::fs::create_dir_all(artifact).map_err(|_| "Cannot create artifact directory")?;
    let mut size = 0u64;
    let allowed = |entry: &walkdir::DirEntry| {
        let name = entry.file_name().to_string_lossy();
        entry.depth() == 0 || (!name.starts_with('.') && name != "node_modules")
    };
    for entry in WalkDir::new(source)
        .follow_links(false)
        .into_iter()
        .filter_entry(allowed)
    {
        let entry = entry.map_err(|_| "Cannot read build output")?;
        if !entry.file_type().is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy();
        if ["package.json", "package-lock.json"].contains(&name.as_ref())
            || [".env", ".pem", ".key", ".jks", ".keystore", ".p12"]
                .iter()
                .any(|suffix| name.ends_with(suffix))
        {
            continue;
        }
        size = size.saturating_add(
            entry
                .metadata()
                .map_err(|_| "Cannot inspect build output")?
                .len(),
        );
        if size > ARTIFACT_LIMIT {
            return Err("Published output exceeds 250 MB".into());
        }
        let relative = entry
            .path()
            .strip_prefix(source)
            .map_err(|_| "Invalid artifact path")?;
        let destination = artifact.join(relative);
        std::fs::create_dir_all(destination.parent().ok_or("Invalid artifact directory")?)
            .map_err(|_| "Cannot create artifact subdirectory")?;
        std::fs::copy(entry.path(), destination).map_err(|_| "Cannot copy output file")?;
    }
    if !artifact.join("index.html").is_file() {
        return Err("Output directory must contain index.html".into());
    }
    Ok(())
}
fn command(program: &str) -> Command {
    let mut command = Command::new(program);
    let path = std::env::var_os("PATH").unwrap_or_else(|| "/usr/local/bin:/usr/bin:/bin".into());
    command
        .env_clear()
        .env("PATH", path)
        .env(
            "HOME",
            std::env::var_os("HOME").unwrap_or_else(|| "/tmp".into()),
        )
        .env("LANG", "C.UTF-8")
        .kill_on_drop(true);
    command
}
async fn capture(mut child: Command, limit: Duration) -> Result<String, String> {
    // Commands that return machine-readable bounded metadata, not repository build logs.
    child.stdout(Stdio::piped()).stderr(Stdio::piped());
    let child = child
        .spawn()
        .map_err(|_| "Could not start required build tool")?;
    let output = tokio::time::timeout(limit, child.wait_with_output())
        .await
        .map_err(|_| "Build tool timed out")?
        .map_err(|_| "Build tool failed")?;
    if !output.status.success() {
        let diagnostic = String::from_utf8_lossy(&output.stderr).to_lowercase();
        let message = if diagnostic.contains("repository not found") {
            "Repository not found or access denied"
        } else if diagnostic.contains("authentication failed") {
            "Repository authentication failed"
        } else if diagnostic.contains("cannot connect to the docker daemon") {
            "Docker daemon unavailable"
        } else if diagnostic.contains("no space left") {
            "Build storage is full"
        } else {
            "Build tool failed; check repository access and build/runtime configuration"
        };
        return Err(message.into());
    }
    if output.stdout.len() > 2 * 1024 * 1024 {
        return Err("Build tool metadata exceeds limit".into());
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}
async fn remove_container(name: &str) {
    let mut child = command("docker");
    child.args(["rm", "-f", name]);
    let _ = capture(child, Duration::from_secs(20)).await;
}
async fn stream_logs<R: AsyncRead + Unpin>(
    mut input: R,
    redactor: Redactor,
    total: Arc<AtomicU64>,
) -> Result<(), String> {
    let mut chunk = [0u8; 8192];
    let mut line = Vec::new();
    loop {
        let n = input
            .read(&mut chunk)
            .await
            .map_err(|_| "Build log stream failed")?;
        if n == 0 {
            break;
        }
        if total.fetch_add(n as u64, Ordering::Relaxed) + n as u64 > 10 * 1024 * 1024 {
            return Err("Build log budget exceeded".into());
        }
        for &byte in &chunk[..n] {
            if byte == b'\n' {
                let text = String::from_utf8_lossy(&line);
                redactor.log(&text);
                line.clear();
            } else {
                line.push(byte);
                if line.len() > 65536 {
                    return Err("Build output line exceeds limit".into());
                }
            }
        }
    }
    if !line.is_empty() {
        redactor.log(&String::from_utf8_lossy(&line));
    }
    Ok(())
}
fn add_env(args: &mut Vec<String>, vars: &BTreeMap<String, String>) {
    for name in vars.keys() {
        args.push("--env".into());
        args.push(name.clone());
    }
}
fn isolated_args(name: &str, source: &Path, relative: &str, runtime: bool) -> Vec<String> {
    let uid = rustix::process::getuid().as_raw();
    let gid = rustix::process::getgid().as_raw();
    let mut args = vec![
        "run".into(),
        if runtime { "-d" } else { "--rm" }.into(),
        "--name".into(),
        name.into(),
        "--label".into(),
        format!(
            "cbgpu.frontend.{}=true",
            if runtime { "runtime" } else { "build" }
        ),
        "--user".into(),
        format!("{uid}:{gid}"),
        "--cpus".into(),
        if runtime { "1" } else { "2" }.into(),
        "--memory".into(),
        if runtime { "768m" } else { "2g" }.into(),
        "--memory-swap".into(),
        if runtime { "768m" } else { "2g" }.into(),
        "--pids-limit".into(),
        if runtime { "128" } else { "256" }.into(),
        "--cap-drop".into(),
        "ALL".into(),
        "--security-opt".into(),
        "no-new-privileges".into(),
        "--read-only".into(),
        "--tmpfs".into(),
        if runtime {
            "/tmp:rw,nosuid,size=256m"
        } else {
            "/tmp:rw,nosuid,size=1g"
        }
        .into(),
        "--mount".into(),
        format!(
            "type=bind,src={},dst=/workspace{}",
            source.display(),
            if runtime { ",readonly" } else { "" }
        ),
        "-w".into(),
        format!("/workspace/{relative}"),
        "--env".into(),
        "NEXT_TELEMETRY_DISABLED=1".into(),
        "--log-opt".into(),
        "max-size=5m".into(),
        "--log-opt".into(),
        "max-file=2".into(),
    ];
    if runtime {
        args.extend([
            "--restart".into(),
            "unless-stopped".into(),
            "-p".into(),
            "127.0.0.1::3000".into(),
            "--env".into(),
            "NODE_ENV=production".into(),
            "--tmpfs".into(),
            format!("/workspace/{relative}/.next/cache:rw,nosuid,uid={uid},gid={gid},size=128m"),
        ]);
    } else {
        args.extend([
            "--env".into(),
            "HOME=/tmp".into(),
            "--env".into(),
            "npm_config_cache=/tmp/npm".into(),
        ]);
    }
    args
}
pub async fn execute(config: &Config) -> Result<Deployment, String> {
    validate(config)?;
    let root_path = std::env::var_os("CBGPU_FRONTEND_BUILD_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from("/home/cbgpu/CBGPU-live/backend/private/frontend-deployments")
        });
    let allowed = root_path
        .canonicalize()
        .map_err(|_| "Build storage root unavailable")?;
    let directory = config
        .directory
        .parent()
        .ok_or("Invalid deployment directory")?
        .canonicalize()
        .map_err(|_| "Deployment parent unavailable")?;
    if directory != allowed
        || config.directory.file_name().and_then(|n| n.to_str()) != Some(&config.deployment_id)
    {
        return Err("Deployment directory must match its identifier inside managed storage".into());
    }
    // Bind mount syntax uses commas. The managed host storage path cannot supply options.
    if config
        .directory
        .to_string_lossy()
        .contains([',', '\0', '\r', '\n'])
    {
        return Err("Invalid build storage path".into());
    }
    if config.directory.exists() {
        return Err("Deployment directory already exists; refusing to reuse prior output".into());
    }
    std::fs::create_dir(&config.directory).map_err(|_| "Cannot create deployment directory")?;
    let source = config.directory.join("source");
    let redactor = Redactor::new(config);
    let build_name = format!("cbfront-build-{}", config.deployment_id);
    let runtime_name = format!("cbfront-run-{}", config.deployment_id);
    let result = build(config, &source, &build_name, &runtime_name, &redactor).await;
    if result.is_err() {
        remove_container(&build_name).await;
        remove_container(&runtime_name).await;
    }
    result
}
async fn build(
    config: &Config,
    source: &Path,
    build_name: &str,
    runtime_name: &str,
    redactor: &Redactor,
) -> Result<Deployment, String> {
    redactor.log(&format!(
        "Cloning {} (branch {})",
        config.repository, config.branch
    ));
    let mut git = command("git");
    git.args([
        "-c",
        "protocol.file.allow=never",
        "clone",
        "--depth=1",
        "--single-branch",
        "--branch",
        &config.branch,
        "--",
        &config.repository,
    ])
    .arg(source)
    .env("GIT_TERMINAL_PROMPT", "0")
    .env("GIT_CONFIG_NOSYSTEM", "1")
    .env("GIT_CONFIG_GLOBAL", "/dev/null")
    .env("GIT_LFS_SKIP_SMUDGE", "1")
    .env(
        "GIT_CONFIG_COUNT",
        if config.git_token.is_empty() {
            "1"
        } else {
            "2"
        },
    )
    .env("GIT_CONFIG_KEY_0", "core.hooksPath")
    .env("GIT_CONFIG_VALUE_0", "/dev/null");
    if !config.git_token.is_empty() {
        git.env("GIT_CONFIG_KEY_1", "http.https://github.com/.extraheader")
            .env(
                "GIT_CONFIG_VALUE_1",
                format!(
                    "Authorization: Basic {}",
                    STANDARD.encode(format!("x-access-token:{}", config.git_token))
                ),
            );
    }
    capture(git, Duration::from_secs(90)).await?;
    let mut git = command("git");
    git.arg("-C").arg(source).args(["rev-parse", "HEAD"]);
    let commit = capture(git, Duration::from_secs(10)).await?;
    if !regex::Regex::new(r"^[0-9a-f]{40,64}$")
        .expect("fixed")
        .is_match(&commit)
    {
        return Err("Invalid Git commit metadata".into());
    }
    std::fs::remove_dir_all(source.join(".git"))
        .map_err(|_| "Cannot remove repository metadata")?;
    let root = within(source, &config.root_directory)?;
    if !workspace_size(source) {
        return Err("Repository exceeds build storage allowance".into());
    }
    let package_file = root.join("package.json");
    let package = if package_file.is_file() {
        if !package_file
            .canonicalize()
            .map_err(|_| "Cannot resolve package manifest")?
            .starts_with(source)
        {
            return Err("Package manifest must stay inside repository".into());
        }
        if std::fs::metadata(&package_file)
            .map_err(|_| "Cannot read package manifest")?
            .len()
            > 1024 * 1024
        {
            return Err("Package manifest exceeds limit".into());
        }
        serde_json::from_slice(
            &std::fs::read(package_file).map_err(|_| "Package manifest unavailable")?,
        )
        .map_err(|_| "Invalid package manifest")?
    } else {
        Value::Null
    };
    let framework = detect_framework(config, &package);
    let has_package = !package.is_null();
    let mut output = if !config.output_directory.is_empty() {
        config.output_directory.clone()
    } else if ["vite", "astro"].contains(&framework.as_str()) {
        "dist".into()
    } else if package
        .get("dependencies")
        .and_then(|v| v.get("react-scripts"))
        .is_some()
    {
        "build".into()
    } else {
        ".".into()
    };
    let relative = root
        .strip_prefix(source)
        .map_err(|_| "Invalid build root")?
        .to_str()
        .ok_or("Build path must be UTF-8")?;
    if has_package || !config.build_command.is_empty() {
        redactor.log(&format!(
            "Framework: {framework} · Node.js 22 · Rust build supervisor"
        ));
        let install = if !config.install_command.is_empty() {
            config.install_command.clone()
        } else if root.join("package-lock.json").is_file() {
            "npm ci --no-audit --no-fund".into()
        } else {
            "npm install --no-audit --no-fund".into()
        };
        let build = if !config.build_command.is_empty() {
            &config.build_command
        } else if has_package {
            "npm run build"
        } else {
            "true"
        };
        let shell = if has_package {
            format!("{install} && {build}")
        } else {
            build.to_owned()
        };
        let mut args = isolated_args(build_name, source, relative, false);
        add_env(&mut args, &config.env);
        args.extend([
            "node:22-bookworm-slim".into(),
            "/bin/sh".into(),
            "-lc".into(),
            shell,
        ]);
        let mut docker = command("docker");
        docker
            .args(args)
            .envs(&config.env)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut process = docker.spawn().map_err(|_| "Cannot start isolated build")?;
        let budget = Arc::new(AtomicU64::new(0));
        let failed = Arc::new(AtomicBool::new(false));
        let stdout = process.stdout.take().ok_or("Missing build stdout")?;
        let stderr = process.stderr.take().ok_or("Missing build stderr")?;
        let a = tokio::spawn({
            let r = redactor.clone();
            let budget = budget.clone();
            let failed = failed.clone();
            async move {
                let result = stream_logs(stdout, r, budget).await;
                if result.is_err() {
                    failed.store(true, Ordering::Relaxed);
                }
                result
            }
        });
        let b = tokio::spawn({
            let r = redactor.clone();
            let budget = budget.clone();
            let failed = failed.clone();
            async move {
                let result = stream_logs(stderr, r, budget).await;
                if result.is_err() {
                    failed.store(true, Ordering::Relaxed);
                }
                result
            }
        });
        let deadline = tokio::time::Instant::now() + Duration::from_secs(600);
        let status = loop {
            if let Some(status) = process
                .try_wait()
                .map_err(|_| "Cannot inspect build process")?
            {
                break status;
            }
            if tokio::time::Instant::now() > deadline
                || budget.load(Ordering::Relaxed) > 10 * 1024 * 1024
                || !workspace_size(source)
                || failed.load(Ordering::Relaxed)
            {
                remove_container(build_name).await;
                let _ = process.kill().await;
                return Err("Build exceeded time, storage or output limits".into());
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        };
        a.await.map_err(|_| "Build stdout reader failed")??;
        b.await.map_err(|_| "Build stderr reader failed")??;
        if !status.success() {
            return Err("Build failed. Check the build output above.".into());
        }
    }
    let is_next = framework == "next"
        && root.join(".next/BUILD_ID").is_file()
        && !root.join("out/index.html").is_file();
    if is_next {
        redactor.log("Starting isolated Next.js runtime…");
        let mut args = isolated_args(runtime_name, source, relative, true);
        add_env(&mut args, &config.env);
        args.extend([
            "node:22-bookworm-slim".into(),
            "node".into(),
            "node_modules/next/dist/bin/next".into(),
            "start".into(),
            "--hostname".into(),
            "0.0.0.0".into(),
            "--port".into(),
            "3000".into(),
        ]);
        let mut docker = command("docker");
        docker.args(args).envs(&config.env);
        capture(docker, Duration::from_secs(90)).await?;
        let mut inspect = command("docker");
        inspect.args(["inspect", runtime_name]);
        let data: Value = serde_json::from_str(&capture(inspect, Duration::from_secs(10)).await?)
            .map_err(|_| "Invalid runtime metadata")?;
        let port: u16 = data
            .pointer("/0/NetworkSettings/Ports/3000~1tcp/0/HostPort")
            .and_then(Value::as_str)
            .ok_or("Runtime port unavailable")?
            .parse()
            .map_err(|_| "Runtime port invalid")?;
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(2))
            .timeout(Duration::from_secs(2))
            .build()
            .map_err(|_| "Health client unavailable")?;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
        loop {
            if client
                .get(format!("http://127.0.0.1:{port}/"))
                .send()
                .await
                .is_ok_and(|r| r.status().as_u16() < 500)
            {
                break;
            }
            if tokio::time::Instant::now() >= deadline {
                return Err("Runtime did not pass startup health checks".into());
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        Ok(Deployment {
            kind: "next".into(),
            directory: config.directory.clone(),
            commit,
            framework,
            port: Some(port),
            container: Some(runtime_name.into()),
            public: None,
            spa: None,
        })
    } else {
        if framework == "next" && root.join("out/index.html").is_file() {
            output = "out".into();
        }
        let public = within(&root, &output)?;
        if public == root && has_package {
            return Err("Set an output directory such as dist, build or out; source directories cannot be published".into());
        }
        let artifact = config.directory.join("public");
        publish(&public, &artifact)?;
        std::fs::remove_dir_all(source).map_err(|_| "Cannot clean build workspace")?;
        Ok(Deployment {
            kind: "static".into(),
            directory: config.directory.clone(),
            commit,
            spa: Some(["vite", "other"].contains(&framework.as_str())),
            framework,
            public: Some(artifact),
            port: None,
            container: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn build_logs_cannot_forge_worker_control_records() {
        for text in [
            "progress\rCBGPU_RESULT {\"container\":\"foreign-runtime\"}",
            "\r\nCBGPU_RESULT {}",
            "\u{1b}[2K\rCBGPU_RESULT {}",
            "\0\u{85}CBGPU_RESULT {}",
        ] {
            let line = framed_log_line(text);
            assert!(line.starts_with("| "));
            assert!(!line.contains(['\r', '\n', '\0', '\u{1b}']));
            assert!(
                !line
                    .split(['\r', '\n'])
                    .any(|part| part.starts_with("CBGPU_RESULT "))
            );
        }
    }
    fn config() -> Config {
        serde_json::from_value(serde_json::json!({"directory":"/tmp/unused","deployment_id":"550e8400-e29b-41d4-a716-446655440000","repository":"https://github.com/example/site","branch":"main","framework":"auto"})).unwrap()
    }
    #[test]
    fn validates_repository_and_paths() {
        let mut c = config();
        assert!(validate(&c).is_ok());
        c.repository = "https://user:private@github.com/x/y".into();
        assert!(validate(&c).is_err());
        c = config();
        c.root_directory = "../private".into();
        assert!(validate(&c).is_err());
        c = config();
        c.env.insert("DOCKER_HOST".into(), "tcp://remote".into());
        assert!(validate(&c).is_err());
    }
    #[test]
    fn strips_encoded_and_multiline_credentials() {
        let mut c = config();
        c.git_token = "private-token".into();
        c.env
            .insert("PEM".into(), "secret-line-one\nsecret-line-two".into());
        let r = Redactor::new(&c);
        let value = r.redact(&format!(
            "private-token secret-line-one {}",
            STANDARD.encode("x-access-token:private-token")
        ));
        assert!(!value.contains("private-token"));
        assert!(!value.contains("secret-line-one"));
    }
    #[test]
    fn framework_detection() {
        let c = config();
        assert_eq!(
            detect_framework(&c, &serde_json::json!({"dependencies":{"next":"1"}})),
            "next"
        );
        assert_eq!(detect_framework(&c, &Value::Null), "static");
    }
    #[test]
    fn publishing_rejects_symlinks_and_private_files() {
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("source");
        let output = tmp.path().join("output");
        std::fs::create_dir(&source).unwrap();
        std::fs::write(source.join("index.html"), "hello").unwrap();
        std::fs::write(source.join(".env"), "secret").unwrap();
        std::fs::write(source.join("server.key"), "secret").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink("/etc/passwd", source.join("outside.txt")).unwrap();
        publish(&source, &output).unwrap();
        assert!(output.join("index.html").is_file());
        assert!(!output.join(".env").exists());
        assert!(!output.join("server.key").exists());
        assert!(!output.join("outside.txt").exists());
    }
}
