pub mod windows;
use axum::{
    Json, Router,
    body::Body,
    extract::{DefaultBodyLimit, State},
    http::{HeaderMap, Request, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    future::Future,
    path::{Path, PathBuf},
    pin::Pin,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
const ORIGIN: &str = "https://cbgpu.com";
const MANIFEST: &str = "https://cbgpu.com/api/compute/installer-manifest";
const IMAGE: &str = "quay.io/jupyter/pytorch-notebook@sha256:83066a1f1108f35cfea7b0f93ae24bb70d6e55c8eeff82b4f9a86dae4496f77b";
type Answer = Result<Json<Value>, StatusCode>;
type RunFuture<'a> = Pin<Box<dyn Future<Output = Result<Vec<u8>, ()>> + Send + 'a>>;
pub trait Runner: Send + Sync {
    fn run<'a>(&'a self, program: &'a Path, args: Vec<String>) -> RunFuture<'a>;
    fn prepare_image<'a>(&'a self, _config: &'a Path) -> RunFuture<'a> {
        Box::pin(async { Err(()) })
    }
}
pub struct RealRunner;
impl Runner for RealRunner {
    fn prepare_image<'a>(&'a self, config: &'a Path) -> RunFuture<'a> {
        Box::pin(async move {
            let mut child = tokio::process::Command::new("/usr/bin/docker")
                .args(["--host", "unix:///var/run/docker.sock", "--config"])
                .arg(config)
                .args(["image", "pull", IMAGE])
                .env_remove("DOCKER_HOST")
                .env_remove("DOCKER_CONTEXT")
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .kill_on_drop(true)
                .spawn()
                .map_err(|_| ())?;
            let status = tokio::time::timeout(Duration::from_secs(900), child.wait())
                .await
                .map_err(|_| ())?
                .map_err(|_| ())?;
            if !status.success() {
                return Err(());
            }
            Ok(Vec::new())
        })
    }
    fn run<'a>(&'a self, program: &'a Path, args: Vec<String>) -> RunFuture<'a> {
        Box::pin(async move {
            use tokio::io::AsyncReadExt;
            let mut command = tokio::process::Command::new(program);
            if program == Path::new("/usr/bin/docker") {
                command
                    .args(["--host", "unix:///var/run/docker.sock"])
                    .env_remove("DOCKER_HOST")
                    .env_remove("DOCKER_CONTEXT");
            }
            let mut child = command
                .args(args)
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .kill_on_drop(true)
                .spawn()
                .map_err(|_| ())?;
            let stdout = child.stdout.take().ok_or(())?;
            let stderr = child.stderr.take().ok_or(())?;
            tokio::time::timeout(Duration::from_secs(60), async {
                let mut out = Vec::new();
                let mut err = Vec::new();
                let mut stdout = stdout.take(65537);
                let mut stderr = stderr.take(65537);
                let (status, a, b) = tokio::join!(
                    child.wait(),
                    stdout.read_to_end(&mut out),
                    stderr.read_to_end(&mut err)
                );
                if !status.map_err(|_| ())?.success()
                    || a.is_err()
                    || b.is_err()
                    || out.len() > 65536
                    || err.len() > 65536
                {
                    return Err(());
                }
                Ok(out)
            })
            .await
            .map_err(|_| ())?
        })
    }
}
#[derive(Default, Clone, Serialize)]
pub struct Progress {
    phase: String,
    message: String,
    enrollment: bool,
    policy: bool,
    doctor: Option<Value>,
    busy: bool,
}
pub struct App {
    #[cfg(test)]
    fake_agent: Option<PathBuf>,
    #[cfg(windows)]
    enrollment_memory: Mutex<Option<Vec<u8>>>,
    #[cfg(windows)]
    policy_memory: Mutex<Option<Vec<u8>>>,
    origin: String,
    bootstrap: String,
    bootstrap_used: AtomicBool,
    access: String,
    script_nonce: String,
    cookie: String,
    csrf: String,
    created: Instant,
    private: PathBuf,
    identity: PathBuf,
    runner: Arc<dyn Runner>,
    state: Mutex<Progress>,
    client: reqwest::Client,
}
fn random() -> String {
    hex::encode(rand::random::<[u8; 32]>())
}
fn private_dir(path: &Path) -> Result<(), ()> {
    for part in path.ancestors() {
        if std::fs::symlink_metadata(part).is_ok_and(|m| m.file_type().is_symlink()) {
            return Err(());
        }
    }
    if let Ok(meta) = std::fs::symlink_metadata(path) {
        if !meta.is_dir() {
            return Err(());
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if meta.permissions().mode() & 0o077 != 0 {
                return Err(());
            }
        }
        return Ok(());
    }
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(path).map_err(|_| ())?;
    #[cfg(windows)]
    {
        let user = std::env::var("USERNAME").map_err(|_| ())?;
        let success =
            std::process::Command::new(windows::system_program("icacls.exe").map_err(|_| ())?)
                .arg(path)
                .args(["/inheritance:r", "/grant:r", &format!("{user}:(OI)(CI)F")])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .map_err(|_| ())?;
        if !success.success() {
            return Err(());
        }
    }
    Ok(())
}
fn write_private(path: &Path, bytes: &[u8], executable: bool) -> Result<(), ()> {
    use std::io::Write;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(if executable { 0o700 } else { 0o600 });
    }
    #[cfg(not(unix))]
    let _ = executable;
    let mut file = options.open(path).map_err(|_| ())?;
    file.write_all(bytes).map_err(|_| ())?;
    file.sync_all().map_err(|_| ())
}
impl App {
    pub fn new(port: u16) -> Result<Self, Box<dyn std::error::Error>> {
        let home = std::env::var_os(if cfg!(windows) {
            "LOCALAPPDATA"
        } else {
            "HOME"
        })
        .ok_or("Home directory unavailable")?;
        Self::with_home(port, Path::new(&home), Arc::new(RealRunner))
            .map_err(|_| "Private setup directory unavailable".into())
    }
    fn with_home(port: u16, home: &Path, runner: Arc<dyn Runner>) -> Result<Self, ()> {
        let base = home.join(if cfg!(windows) {
            "CBGPU/NodeSetup"
        } else {
            ".config/cbgpu-node-setup"
        });
        private_dir(&base)?;
        let private = base.join(random());
        private_dir(&private)?;
        Ok(Self {
            #[cfg(test)]
            fake_agent: None,
            #[cfg(windows)]
            enrollment_memory: Mutex::new(None),
            #[cfg(windows)]
            policy_memory: Mutex::new(None),
            origin: format!("http://127.0.0.1:{port}"),
            bootstrap: random(),
            bootstrap_used: AtomicBool::new(false),
            access: random(),
            script_nonce: random(),
            cookie: random(),
            csrf: random(),
            created: Instant::now(),
            private,
            identity: home.join(".config/cbgpu-node/identity.json"),
            runner,
            state: Mutex::new(Progress {
                phase: "ready".into(),
                message: "검사를 실행하기 전에 아래 내용을 확인해 주세요.".into(),
                ..Default::default()
            }),
            client: reqwest::Client::builder()
                .https_only(true)
                .redirect(reqwest::redirect::Policy::none())
                .timeout(Duration::from_secs(90))
                .build()
                .map_err(|_| ())?,
        })
    }
    /// One-use capability is kept in a private file, never a URL, command argument or log.
    pub fn launcher_file(&self) -> std::io::Result<PathBuf> {
        let file = self.private.join("open-setup.html");
        let html = format!(
            "<!doctype html><meta charset=utf-8><title>CBGPU 서버 연결</title><form method=post action=\"{}/start\"><input type=hidden name=capability value=\"{}\"><button>CBGPU 서버 연결 열기</button></form><script>document.forms[0].submit()</script>",
            self.origin, self.bootstrap
        );
        write_private(&file, html.as_bytes(), false).map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "Private browser launcher unavailable",
            )
        })?;
        Ok(file)
    }
    fn alive(&self) -> bool {
        self.created.elapsed() < Duration::from_secs(1200)
    }
    fn update(&self, phase: &str, message: &str) {
        let mut s = self.state.lock().unwrap();
        s.phase = phase.into();
        s.message = message.into();
    }
    fn busy(&self) -> Result<(), StatusCode> {
        let mut s = self.state.lock().unwrap();
        if s.busy {
            return Err(StatusCode::CONFLICT);
        }
        s.busy = true;
        Ok(())
    }
}
impl Drop for App {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.private);
    }
}
fn cookie_valid(headers: &HeaderMap, app: &App) -> bool {
    headers
        .get(header::COOKIE)
        .and_then(|h| h.to_str().ok())
        .is_some_and(|s| {
            s.split(';')
                .any(|p| p.trim() == format!("cbgpu_setup={}", app.cookie))
        })
}
async fn boundary(State(app): State<Arc<App>>, request: Request<Body>, next: Next) -> Response {
    let host = request
        .headers()
        .get(header::HOST)
        .and_then(|v| v.to_str().ok());
    if !app.alive() || host != app.origin.strip_prefix("http://") {
        return StatusCode::FORBIDDEN.into_response();
    }
    let path = request.uri().path();
    if request.uri().query().is_some() {
        return StatusCode::FORBIDDEN.into_response();
    }
    let bootstrap = path == "/start" && request.method() == axum::http::Method::POST;
    if !bootstrap && !cookie_valid(request.headers(), &app) {
        return StatusCode::FORBIDDEN.into_response();
    }
    if path.starts_with("/api/")
        && request
            .headers()
            .get("x-cbgpu-setup-access")
            .and_then(|v| v.to_str().ok())
            != Some(app.access.as_str())
    {
        return StatusCode::FORBIDDEN.into_response();
    }
    if !bootstrap
        && request.method() != axum::http::Method::GET
        && (request
            .headers()
            .get(header::ORIGIN)
            .and_then(|v| v.to_str().ok())
            != Some(app.origin.as_str())
            || request
                .headers()
                .get("x-cbgpu-csrf")
                .and_then(|v| v.to_str().ok())
                != Some(app.csrf.as_str()))
    {
        return StatusCode::FORBIDDEN.into_response();
    }
    let mut response = next.run(request).await;
    let h = response.headers_mut();
    for (name, value) in [
        ("cache-control", "no-store"),
        ("x-content-type-options", "nosniff"),
        ("referrer-policy", "no-referrer"),
        (
            "content-security-policy",
            "default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self'; connect-src 'self'; form-action 'self'; frame-ancestors 'none'; base-uri 'none'",
        ),
    ] {
        h.insert(name, value.parse().unwrap());
    }
    h.insert("content-security-policy",format!("default-src 'none'; script-src 'self' 'nonce-{}'; style-src 'self'; img-src 'self'; connect-src 'self'; form-action 'self'; frame-ancestors 'none'; base-uri 'none'",app.script_nonce).parse().unwrap());
    response
}
async fn start(State(app): State<Arc<App>>, request: Request<Body>) -> Response {
    let origin = request
        .headers()
        .get(header::ORIGIN)
        .and_then(|v| v.to_str().ok());
    if !matches!(origin, None | Some("null")) {
        return StatusCode::FORBIDDEN.into_response();
    }
    if !request
        .headers()
        .get(header::CONTENT_TYPE)
        .is_some_and(|v| {
            v.as_bytes()
                .starts_with(b"application/x-www-form-urlencoded")
        })
    {
        return StatusCode::FORBIDDEN.into_response();
    }
    let Ok(bytes) = axum::body::to_bytes(request.into_body(), 256).await else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    let Ok(text) = std::str::from_utf8(&bytes) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    if text != format!("capability={}", app.bootstrap)
        || app.bootstrap_used.swap(true, Ordering::SeqCst)
    {
        return StatusCode::FORBIDDEN.into_response();
    }
    let _ = std::fs::remove_file(app.private.join("open-setup.html"));
    (
        [(header::CONTENT_TYPE,"text/html; charset=utf-8".to_owned()),
         (header::SET_COOKIE,format!("cbgpu_setup={}; HttpOnly; SameSite=Strict; Path=/; Max-Age=1200",app.cookie))],
        format!("<!doctype html><meta charset=utf-8><title>CBGPU 서버 연결</title><p>연결 도우미를 열고 있습니다.</p><script nonce=\"{}\">sessionStorage.setItem('cbgpu-setup-access','{}');location.replace('/')</script>",app.script_nonce,app.access)
    ).into_response()
}
pub fn router(app: Arc<App>) -> Router {
    Router::new()
        .route("/start", post(start))
        .route(
            "/",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
                    include_str!("../web/dist/index.html"),
                )
            }),
        )
        .route(
            "/app.js",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "text/javascript")],
                    include_str!("../web/dist/app.js"),
                )
            }),
        )
        .route(
            "/style.css",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "text/css")],
                    include_str!("../web/style.css"),
                )
            }),
        )
        .route(
            "/logo.svg",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "image/svg+xml")],
                    include_str!("../web/logo.svg"),
                )
            }),
        )
        .route("/api/state", get(state))
        .route("/api/enrollment", post(enrollment))
        .route("/api/policy", post(policy))
        .route("/api/doctor", post(doctor))
        .route("/api/prepare-image", post(prepare_image))
        .route("/api/install", post(install))
        .layer(DefaultBodyLimit::max(16384))
        .layer(middleware::from_fn_with_state(app.clone(), boundary))
        .with_state(app)
}
async fn state(State(app): State<Arc<App>>) -> Json<Value> {
    let progress = app.state.lock().unwrap().clone();
    Json(
        json!({"csrf":app.csrf,"os":std::env::consts::OS,"architecture":std::env::consts::ARCH,"linux_supported":cfg!(target_os="linux")&&std::env::consts::ARCH=="x86_64","progress":progress,"image":IMAGE,"windows_install_validated":windows::VALIDATED}),
    )
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Enrollment {
    node_id: String,
    school_id: String,
    token: String,
    expires_at: String,
    sharing_enabled: bool,
}
fn valid_uuid(s: &str) -> bool {
    s.len() == 36
        && s.chars().enumerate().all(|(i, c)| {
            if [8, 13, 18, 23].contains(&i) {
                c == '-'
            } else {
                c.is_ascii_hexdigit()
            }
        })
}
async fn enrollment(State(app): State<Arc<App>>, Json(value): Json<Enrollment>) -> Answer {
    app.busy()?;
    let result = (|| {
        let expires = chrono::DateTime::parse_from_rfc3339(&value.expires_at)
            .map_err(|_| StatusCode::BAD_REQUEST)?
            .timestamp();
        let now = chrono::Utc::now().timestamp();
        if !valid_uuid(&value.node_id)
            || value.school_id.is_empty()
            || value.school_id.len() > 100
            || !value
                .school_id
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c))
            || value.sharing_enabled
            || !value.token.starts_with("gpuenr_")
            || value.token.len() != 71
            || !value.token[7..].bytes().all(|b| b.is_ascii_hexdigit())
            || expires <= now
            || expires > now + 900
        {
            return Err(StatusCode::BAD_REQUEST);
        }
        let bytes = serde_json::to_vec(&value).unwrap();
        #[cfg(windows)]
        {
            *app.enrollment_memory.lock().unwrap() = Some(bytes);
        }
        #[cfg(not(windows))]
        {
            let path = app.private.join("enrollment.json");
            if path.exists() {
                std::fs::remove_file(&path).map_err(|_| StatusCode::CONFLICT)?;
            }
            write_private(&path, &bytes, false).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        }
        app.state.lock().unwrap().enrollment = true;
        Ok(Json(json!({"ok":true})))
    })();
    app.state.lock().unwrap().busy = false;
    result
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Policy {
    protocol: u16,
    enabled: bool,
    server_public_key: String,
    image_digest: String,
}
async fn policy(State(app): State<Arc<App>>, Json(value): Json<Policy>) -> Answer {
    app.busy()?;
    let result = (|| {
        if value.protocol != 3
            || !value.enabled
            || value.image_digest != IMAGE
            || value.server_public_key.len() != 64
            || !value
                .server_public_key
                .bytes()
                .all(|b| b.is_ascii_hexdigit())
        {
            return Err(StatusCode::BAD_REQUEST);
        }
        let bytes = serde_json::to_vec(&value).unwrap();
        #[cfg(windows)]
        {
            *app.policy_memory.lock().unwrap() = Some(bytes);
        }
        #[cfg(not(windows))]
        {
            let path = app.private.join("policy.json");
            if path.exists() {
                std::fs::remove_file(&path).map_err(|_| StatusCode::CONFLICT)?;
            }
            write_private(&path, &bytes, false).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        }
        app.state.lock().unwrap().policy = true;
        Ok(Json(json!({"ok":true})))
    })();
    app.state.lock().unwrap().busy = false;
    result
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Consent {
    confirmed: bool,
    #[serde(default)]
    notebooks: bool,
}
#[derive(Deserialize)]
struct Manifest {
    revision: String,
    artifacts: Vec<Artifact>,
}
#[derive(Deserialize)]
struct Artifact {
    platform: String,
    url: String,
    sha256: String,
    size: usize,
    filename: String,
}
fn artifact(m: Manifest) -> Result<Artifact, ()> {
    if m.revision.len() != 40 || !m.revision.bytes().all(|c| c.is_ascii_hexdigit()) {
        return Err(());
    }
    let a = m
        .artifacts
        .into_iter()
        .find(|a| a.platform == "linux-x86_64")
        .ok_or(())?;
    if a.filename != "cbgpu-node-linux-x86_64"
        || a.url
            != format!(
                "{ORIGIN}/api/compute/downloads/{}/cbgpu-node-linux-x86_64",
                m.revision
            )
        || a.sha256.len() != 64
        || !a.sha256.bytes().all(|c| c.is_ascii_hexdigit())
        || a.size == 0
        || a.size > 33554432
    {
        return Err(());
    }
    Ok(a)
}
async fn bounded(mut response: reqwest::Response, max: usize) -> Result<Vec<u8>, ()> {
    if !response.status().is_success() || response.content_length().is_some_and(|s| s > max as u64)
    {
        return Err(());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| ())? {
        if bytes.len() + chunk.len() > max {
            return Err(());
        }
        bytes.extend(chunk)
    }
    Ok(bytes)
}
async fn agent(app: &App) -> Result<PathBuf, ()> {
    #[cfg(test)]
    if let Some(path) = &app.fake_agent {
        return Ok(path.clone());
    }
    let manifest: Manifest = serde_json::from_slice(
        &bounded(
            app.client.get(MANIFEST).send().await.map_err(|_| ())?,
            32768,
        )
        .await?,
    )
    .map_err(|_| ())?;
    let artifact = artifact(manifest)?;
    let bytes = bounded(
        app.client.get(&artifact.url).send().await.map_err(|_| ())?,
        artifact.size,
    )
    .await?;
    if bytes.len() != artifact.size || hex::encode(Sha256::digest(&bytes)) != artifact.sha256 {
        return Err(());
    }
    let path = app.private.join("cbgpu-node");
    if path.exists() {
        std::fs::remove_file(&path).map_err(|_| ())?;
    }
    write_private(&path, &bytes, true)?;
    Ok(path)
}
fn safe_doctor(raw: &[u8]) -> Result<Value, ()> {
    let value: Value = serde_json::from_slice(raw).map_err(|_| ())?;
    let mut out = serde_json::Map::new();
    for field in [
        "ubuntu",
        "existing_nvidia_driver_ready",
        "user_systemd_ready",
        "user_lingering",
        "persistence_prerequisites_ready",
        "execution_ready",
        "drivers_changed",
    ] {
        if let Some(v) = value.get(field).filter(|v| v.is_boolean() || v.is_null()) {
            out.insert(field.into(), v.clone());
        }
    }
    if let Some(v) = value
        .get("architecture")
        .and_then(Value::as_str)
        .filter(|v| ["AMD64", "ARM64", "UNSUPPORTED"].contains(v))
    {
        out.insert("architecture".into(), json!(v));
    }
    if out.get("ubuntu").is_none() {
        return Err(());
    }
    Ok(Value::Object(out))
}
async fn prepare_image(State(app): State<Arc<App>>, Json(consent): Json<Consent>) -> Answer {
    if !consent.confirmed {
        return Err(StatusCode::BAD_REQUEST);
    }
    if !cfg!(target_os = "linux") || std::env::consts::ARCH != "x86_64" {
        return Err(StatusCode::NOT_IMPLEMENTED);
    }
    app.busy()?;
    tokio::spawn(async move {
        app.update("preparing_image","고정된 Jupyter 이미지를 준비하고 있습니다. 다운로드 환경에 따라 시간이 걸릴 수 있습니다.");
        let result = async {
            let docker = Path::new("/usr/bin/docker");
            app.runner
                .run(
                    docker,
                    vec![
                        "version".into(),
                        "--format".into(),
                        "{{.Server.Version}}".into(),
                    ],
                )
                .await?;
            let config = app.private.join("docker-public-image-config");
            private_dir(&config)?;
            let file = config.join("config.json");
            if !file.exists() {
                write_private(&file, b"{\"auths\":{}}", false)?;
            }
            app.runner.prepare_image(&config).await?;
            app.runner
                .run(
                    docker,
                    vec![
                        "image".into(),
                        "inspect".into(),
                        IMAGE.into(),
                        "--format".into(),
                        "{{.Id}}".into(),
                    ],
                )
                .await?;
            Ok::<(), ()>(())
        }
        .await;
        app.state.lock().unwrap().doctor = None;
        match result {Ok(())=>app.update("image_ready","Jupyter 이미지가 준비되었습니다. 새 연결 파일을 받고 환경 검사를 진행해 주세요."),Err(())=>app.update("error","이미지를 준비하지 못했습니다. 이 컴퓨터의 Docker 권한, 네트워크와 저장 공간을 확인해 주세요.")}
        app.state.lock().unwrap().busy = false;
    });
    Ok(Json(json!({"accepted":true})))
}
async fn doctor(State(app): State<Arc<App>>, Json(consent): Json<Consent>) -> Answer {
    if !consent.confirmed {
        return Err(StatusCode::BAD_REQUEST);
    }
    app.busy()?;
    let copy = app.clone();
    tokio::spawn(async move {
        app.update(
            "checking",
            "요청한 읽기 전용 환경 검사를 실행하고 있습니다.",
        );
        let result = async {
            if cfg!(windows) {
                return windows::diagnose().await;
            }
            if !cfg!(target_os = "linux") || std::env::consts::ARCH != "x86_64" {
                return Err(());
            }
            let binary = agent(&app).await?;
            {
                let mut value =
                    safe_doctor(&app.runner.run(&binary, vec!["diagnose".into()]).await?)?;
                if consent.notebooks {
                    let docker = Path::new("/usr/bin/docker");
                    let available = app
                        .runner
                        .run(
                            docker,
                            vec![
                                "version".into(),
                                "--format".into(),
                                "{{.Server.Version}}".into(),
                            ],
                        )
                        .await
                        .is_ok();
                    let image = app
                        .runner
                        .run(
                            docker,
                            vec![
                                "image".into(),
                                "inspect".into(),
                                IMAGE.into(),
                                "--format".into(),
                                "{{.Id}}".into(),
                            ],
                        )
                        .await
                        .is_ok();
                    let toolkit = app
                        .runner
                        .run(
                            docker,
                            vec![
                                "info".into(),
                                "--format".into(),
                                "{{json .Runtimes}}".into(),
                            ],
                        )
                        .await
                        .ok()
                        .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
                        .is_some_and(|v| v.get("nvidia").is_some());
                    value["docker_access"] = json!(available);
                    value["approved_image_present"] = json!(image);
                    value["nvidia_runtime_configured"] = json!(toolkit);
                }
                Ok(value)
            }
        }
        .await;
        match result{Ok(value)=>{app.state.lock().unwrap().doctor=Some(value);app.update("checked","검사가 끝났습니다. 설치 및 GPU 실행 승인은 별도입니다.")},Err(())=>app.update("error","환경 검사를 완료하지 못했습니다. Ubuntu, 드라이버 및 공식 다운로드 연결을 확인해 주세요.")}
        app.state.lock().unwrap().busy = false;
    });
    Ok(Json(
        json!({"accepted":true,"state":copy.state.lock().unwrap().phase}),
    ))
}
async fn install(State(app): State<Arc<App>>, Json(consent): Json<Consent>) -> Answer {
    if !consent.confirmed {
        return Err(StatusCode::BAD_REQUEST);
    }
    if cfg!(windows) {
        if !windows::VALIDATED {
            return Err(StatusCode::NOT_IMPLEMENTED);
        }
        app.busy()?;
        tokio::spawn(async move {
            app.update("installing", "WSL2 Ubuntu 연결을 준비하고 있습니다.");
            let result = async {
                let path = agent(&app).await?;
                let bytes = std::fs::read(path).map_err(|_| ())?;
                #[cfg(windows)]
                let enrollment = app.enrollment_memory.lock().unwrap().clone().ok_or(())?;
                #[cfg(windows)]
                let policy = if consent.notebooks {
                    Some(app.policy_memory.lock().unwrap().clone().ok_or(())?)
                } else {
                    None
                };
                #[cfg(not(windows))]
                let (enrollment, policy): (Vec<u8>, Option<Vec<u8>>) = (Vec::new(), None);
                windows::install(
                    &bytes,
                    &hex::encode(Sha256::digest(&bytes)),
                    &enrollment,
                    policy.as_deref(),
                )
                .await
            }
            .await;
            match result { Ok(())=>app.update("installed","WSL2 사용자 서비스 설치가 완료되었습니다. 실행 준비 상태는 서버에서 별도 확인합니다."), Err(())=>app.update("error","WSL2 설치를 완료하지 못했습니다. 기존 설정은 교체하지 않습니다.") }
            app.state.lock().unwrap().busy = false;
        });
        return Ok(Json(json!({"accepted":true})));
    }
    if !cfg!(target_os = "linux") || std::env::consts::ARCH != "x86_64" {
        return Err(StatusCode::NOT_IMPLEMENTED);
    }
    {
        let s = app.state.lock().unwrap();
        if !s.enrollment || consent.notebooks && !s.policy {
            return Err(StatusCode::BAD_REQUEST);
        }
        let d = s.doctor.as_ref().ok_or(StatusCode::PRECONDITION_FAILED)?;
        if d["ubuntu"] != true
            || d["existing_nvidia_driver_ready"] != true
            || d["user_systemd_ready"] != true
            || consent.notebooks
                && (d["docker_access"] != true
                    || d["approved_image_present"] != true
                    || d["nvidia_runtime_configured"] != true)
        {
            return Err(StatusCode::PRECONDITION_FAILED);
        }
    }
    app.busy()?;
    tokio::spawn(async move {
        app.update(
            "installing",
            "서버 등록 및 새 사용자 서비스를 설치하고 있습니다.",
        );
        let result = async {
            if app.identity.exists() {
                return Err(());
            }
            private_dir(app.identity.parent().ok_or(())?)?;
            let binary = agent(&app).await?;
            let enrollment = app.private.join("enrollment.json");
            let parsed: Enrollment =
                serde_json::from_slice(&std::fs::read(&enrollment).map_err(|_| ())?)
                    .map_err(|_| ())?;
            if chrono::DateTime::parse_from_rfc3339(&parsed.expires_at)
                .map_err(|_| ())?
                .timestamp()
                <= chrono::Utc::now().timestamp()
            {
                return Err(());
            }
            let identity = app.identity.to_str().ok_or(())?.to_owned();
            app.runner
                .run(
                    &binary,
                    vec![
                        "init".into(),
                        ORIGIN.into(),
                        enrollment.to_str().ok_or(())?.into(),
                        identity.clone(),
                    ],
                )
                .await?;
            let _ = std::fs::remove_file(enrollment);
            let args = if consent.notebooks {
                let policy = app
                    .identity
                    .parent()
                    .ok_or(())?
                    .join("notebook-policy.json");
                write_private(
                    &policy,
                    &std::fs::read(app.private.join("policy.json")).map_err(|_| ())?,
                    false,
                )?;
                vec![
                    "install-executor-user-service".into(),
                    identity,
                    policy.to_str().ok_or(())?.into(),
                    "--apply".into(),
                ]
            } else {
                vec!["install-user-service".into(), identity, "--apply".into()]
            };
            app.runner.run(&binary, args).await?;
            Ok::<(), ()>(())
        }
        .await;
        match result{Ok(())=>app.update("installed","새 사용자 서비스 설치가 완료되었습니다. 실제 연결 상태는 학교 관리 화면에서 확인해 주세요. GPU 실행 준비 완료를 의미하지는 않습니다."),Err(())=>app.update("error","설치를 완료하지 못했습니다. 기존 설정은 덮어쓰지 않습니다. 등록이 이미 처리되었을 수 있으므로 학교 관리 화면과 로컬 identity 파일을 확인한 후 재시도해 주세요.")}
        app.state.lock().unwrap().busy = false;
    });
    Ok(Json(json!({"accepted":true})))
}
#[cfg(test)]
mod tests {
    use super::*;
    use http_body_util::BodyExt;
    use tower::ServiceExt;
    struct Fake {
        calls: Mutex<Vec<Vec<String>>>,
    }
    impl Runner for Fake {
        fn run<'a>(&'a self, _: &'a Path, args: Vec<String>) -> RunFuture<'a> {
            self.calls.lock().unwrap().push(args.clone());
            Box::pin(async move {
                if args == ["diagnose"] {
                    Ok(serde_json::to_vec(&json!({"ubuntu":true,"architecture":"AMD64","existing_nvidia_driver_ready":true,"user_systemd_ready":true,"execution_ready":false,"secret":"never expose"})).unwrap())
                } else {
                    Ok(b"{}".to_vec())
                }
            })
        }
    }
    fn fixture() -> (Arc<App>, Arc<Fake>) {
        let home = std::env::temp_dir().join(format!("cbgpu-setup-test-{}", random()));
        std::fs::create_dir(&home).unwrap();
        let fake = Arc::new(Fake {
            calls: Mutex::new(vec![]),
        });
        let mut app = App::with_home(19999, &home, fake.clone()).unwrap();
        let binary = app.private.join("fake-reviewed-agent");
        write_private(&binary, b"synthetic-not-executable-in-tests", true).unwrap();
        app.fake_agent = Some(binary);
        (Arc::new(app), fake)
    }
    async fn call(
        app: &Arc<App>,
        method: &str,
        path: &str,
        body: Value,
        authorized: bool,
    ) -> Response {
        let mut req = Request::builder()
            .method(method)
            .uri(path)
            .header("host", "127.0.0.1:19999")
            .header("content-type", "application/json");
        if authorized {
            req = req
                .header("cookie", format!("cbgpu_setup={}", app.cookie))
                .header("origin", &app.origin)
                .header("x-cbgpu-csrf", &app.csrf)
                .header("x-cbgpu-setup-access", &app.access)
        }
        router(app.clone())
            .oneshot(req.body(Body::from(body.to_string())).unwrap())
            .await
            .unwrap()
    }
    fn valid_enrollment() -> Value {
        json!({"node_id":"11111111-1111-4111-8111-111111111111","school_id":"synthetic-school","token":format!("gpuenr_{}","a".repeat(64)),"expires_at":(chrono::Utc::now()+chrono::Duration::minutes(10)).to_rfc3339(),"sharing_enabled":false})
    }
    #[tokio::test]
    async fn loopback_origin_csrf_and_cookie_are_all_required() {
        let (app, fake) = fixture();
        assert_eq!(
            call(
                &app,
                "POST",
                "/api/doctor",
                json!({"confirmed":true}),
                false
            )
            .await
            .status(),
            403
        );
        let req = Request::builder()
            .uri("/")
            .header("host", "attacker.example")
            .body(Body::empty())
            .unwrap();
        assert_eq!(
            router(app.clone()).oneshot(req).await.unwrap().status(),
            403
        );
        let req = Request::builder()
            .method("POST")
            .uri("/api/doctor")
            .header("host", "127.0.0.1:19999")
            .header("cookie", format!("cbgpu_setup={}", app.cookie))
            .header("origin", "https://evil.example")
            .header("x-cbgpu-csrf", &app.csrf)
            .body(Body::empty())
            .unwrap();
        assert_eq!(
            router(app.clone()).oneshot(req).await.unwrap().status(),
            403
        );
        assert!(fake.calls.lock().unwrap().is_empty());
    }
    #[tokio::test]
    async fn enrollment_is_private_validated_and_never_in_state() {
        let (app, _) = fixture();
        let value = valid_enrollment();
        assert_eq!(
            call(&app, "POST", "/api/enrollment", value.clone(), true)
                .await
                .status(),
            200
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(app.private.join("enrollment.json"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
        let response = call(&app, "GET", "/api/state", json!({}), true).await;
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert!(!String::from_utf8_lossy(&body).contains(value["token"].as_str().unwrap()));
        let mut expired = value;
        expired["expires_at"] =
            json!((chrono::Utc::now() - chrono::Duration::minutes(1)).to_rfc3339());
        assert_eq!(
            call(&app, "POST", "/api/enrollment", expired, true)
                .await
                .status(),
            400
        );
    }
    #[cfg(not(windows))]
    #[tokio::test]
    async fn doctor_needs_explicit_consent_and_uses_only_reviewed_diagnose() {
        let (app, fake) = fixture();
        assert_eq!(
            call(
                &app,
                "POST",
                "/api/doctor",
                json!({"confirmed":false}),
                true
            )
            .await
            .status(),
            400
        );
        assert_eq!(
            call(&app, "POST", "/api/doctor", json!({"confirmed":true}), true)
                .await
                .status(),
            200
        );
        for _ in 0..100 {
            if !app.state.lock().unwrap().busy {
                break;
            }
            tokio::task::yield_now().await;
        }
        let state = app.state.lock().unwrap().clone();
        assert_eq!(state.phase, "checked");
        assert!(state.doctor.unwrap().get("secret").is_none());
        assert_eq!(
            *fake.calls.lock().unwrap(),
            vec![vec!["diagnose".to_string()]]
        );
    }
    #[cfg(not(windows))]
    #[tokio::test]
    async fn installation_is_explicit_and_existing_identity_is_not_overwritten() {
        let (app, fake) = fixture();
        assert_eq!(
            call(
                &app,
                "POST",
                "/api/install",
                json!({"confirmed":false}),
                true
            )
            .await
            .status(),
            400
        );
        assert_eq!(
            call(
                &app,
                "POST",
                "/api/install",
                json!({"confirmed":true}),
                true
            )
            .await
            .status(),
            400
        );
        private_dir(app.identity.parent().unwrap()).unwrap();
        write_private(&app.identity, b"existing-owned-identity", false).unwrap();
        {
            let mut state = app.state.lock().unwrap();
            state.enrollment = true;
            state.doctor = Some(
                json!({"ubuntu":true,"existing_nvidia_driver_ready":true,"user_systemd_ready":true}),
            );
        }
        assert_eq!(
            call(
                &app,
                "POST",
                "/api/install",
                json!({"confirmed":true}),
                true
            )
            .await
            .status(),
            200
        );
        for _ in 0..100 {
            if !app.state.lock().unwrap().busy {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert_eq!(
            std::fs::read(&app.identity).unwrap(),
            b"existing-owned-identity"
        );
        assert!(fake.calls.lock().unwrap().is_empty());
    }
    #[test]
    fn manifest_does_not_allow_external_hosts_paths_or_unbounded_artifacts() {
        let revision = "a".repeat(40);
        let value = json!({"revision":revision,"artifacts":[{"platform":"linux-x86_64","url":format!("https://cbgpu.com/api/compute/downloads/{revision}/cbgpu-node-linux-x86_64"),"sha256":"b".repeat(64),"size":123,"filename":"cbgpu-node-linux-x86_64"}]});
        assert!(artifact(serde_json::from_value(value.clone()).unwrap()).is_ok());
        let mut bad = value;
        bad["artifacts"][0]["url"] = json!("https://evil.example/agent");
        assert!(artifact(serde_json::from_value(bad).unwrap()).is_err());
    }
    #[test]
    fn existing_directory_permissions_are_not_modified() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let path = std::env::temp_dir().join(random());
            std::fs::create_dir(&path).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
            assert!(private_dir(&path).is_err());
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o755
            );
            std::fs::remove_dir(path).unwrap();
        }
    }
    #[tokio::test]
    async fn private_browser_launcher_is_required_and_cannot_be_replayed() {
        let (app, _) = fixture();
        let file = app.launcher_file().unwrap();
        let html = std::fs::read_to_string(&file).unwrap();
        assert!(html.contains(&format!("value=\"{}\"", app.bootstrap)));
        assert!(!html.contains(&format!("?{}", app.bootstrap)));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&file).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        let root = call(&app, "GET", "/", json!({}), false).await;
        assert_eq!(root.status(), 403);
        assert!(root.headers().get(header::SET_COOKIE).is_none());
        async fn launch(app: &Arc<App>, cap: &str) -> Response {
            let req = Request::builder()
                .method("POST")
                .uri("/start")
                .header("host", "127.0.0.1:19999")
                .header("origin", "null")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(format!("capability={cap}")))
                .unwrap();
            router(app.clone()).oneshot(req).await.unwrap()
        }
        assert_eq!(launch(&app, &"f".repeat(64)).await.status(), 403);
        let response = launch(&app, &app.bootstrap).await;
        assert_eq!(response.status(), 200);
        assert!(response.headers().get(header::LOCATION).is_none());
        let cookie = response.headers()[header::SET_COOKIE].to_str().unwrap();
        assert!(cookie.contains("HttpOnly; SameSite=Strict"));
        assert!(!cookie.contains("Domain="));
        assert!(!file.exists());
        assert_eq!(launch(&app, &app.bootstrap).await.status(), 403);
    }
    #[tokio::test]
    async fn stolen_cross_port_cookie_cannot_read_state_or_install() {
        let (app, fake) = fixture();
        for (method, path) in [("GET", "/api/state"), ("POST", "/api/install")] {
            let req = Request::builder()
                .method(method)
                .uri(path)
                .header("host", "127.0.0.1:19999")
                .header("origin", &app.origin)
                .header("cookie", format!("cbgpu_setup={}", app.cookie))
                .header("x-cbgpu-csrf", &app.csrf)
                .header("content-type", "application/json")
                .body(Body::from("{\"confirmed\":true}"))
                .unwrap();
            let response = router(app.clone()).oneshot(req).await.unwrap();
            assert_eq!(response.status(), 403);
        }
        let root = call(&app, "GET", "/", json!({}), true).await;
        assert_eq!(root.status(), 200);
        let body = root.into_body().collect().await.unwrap().to_bytes();
        assert!(!String::from_utf8_lossy(&body).contains(&app.access));
        assert!(fake.calls.lock().unwrap().is_empty());
    }
    #[cfg(not(windows))]
    #[tokio::test]
    async fn public_image_preparation_requires_explicit_action_and_only_fixed_image() {
        struct ImageRunner {
            calls: Mutex<Vec<Vec<String>>>,
        }
        impl Runner for ImageRunner {
            fn run<'a>(&'a self, program: &'a Path, args: Vec<String>) -> RunFuture<'a> {
                assert_eq!(program, Path::new("/usr/bin/docker"));
                self.calls.lock().unwrap().push(args);
                Box::pin(async { Ok(Vec::new()) })
            }
            fn prepare_image<'a>(&'a self, config: &'a Path) -> RunFuture<'a> {
                assert_eq!(
                    std::fs::read(config.join("config.json")).unwrap(),
                    b"{\"auths\":{}}"
                );
                self.calls
                    .lock()
                    .unwrap()
                    .push(vec!["reviewed-public-image-pull".into(), IMAGE.into()]);
                Box::pin(async { Ok(Vec::new()) })
            }
        }
        let home = std::env::temp_dir().join(format!("cbgpu-image-test-{}", random()));
        std::fs::create_dir(&home).unwrap();
        let fake = Arc::new(ImageRunner {
            calls: Mutex::new(vec![]),
        });
        let app = Arc::new(App::with_home(19999, &home, fake.clone()).unwrap());
        assert_eq!(
            call(
                &app,
                "POST",
                "/api/prepare-image",
                json!({"confirmed":false}),
                true
            )
            .await
            .status(),
            400
        );
        assert!(fake.calls.lock().unwrap().is_empty());
        assert_eq!(
            call(
                &app,
                "POST",
                "/api/prepare-image",
                json!({"confirmed":true}),
                true
            )
            .await
            .status(),
            200
        );
        for _ in 0..100 {
            if !app.state.lock().unwrap().busy {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert_eq!(app.state.lock().unwrap().phase, "image_ready");
        assert!(app.state.lock().unwrap().doctor.is_none());
        assert_eq!(
            *fake.calls.lock().unwrap(),
            vec![
                vec!["version", "--format", "{{.Server.Version}}"],
                vec!["reviewed-public-image-pull", IMAGE],
                vec!["image", "inspect", IMAGE, "--format", "{{.Id}}"]
            ]
        );
    }
}
