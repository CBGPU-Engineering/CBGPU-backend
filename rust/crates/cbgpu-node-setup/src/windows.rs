//! Experimental Windows -> existing WSL2 Ubuntu adapter.
//! No shell, elevation, WSL installation, driver changes, Docker-group changes or Windows token files.
use sha2::{Digest, Sha256};
use std::{future::Future, pin::Pin};
#[cfg(target_os = "windows")]
use std::{process::Stdio, time::Duration};
#[cfg(target_os = "windows")]
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// No Windows host/physical GPU validation has been performed. The GUI must keep installation disabled.
pub const VALIDATED: bool = false;
const MAX_OUTPUT: usize = 65_536;
const MAX_AGENT: usize = 64 * 1024 * 1024;
const IMAGE: &str = "quay.io/jupyter/pytorch-notebook@sha256:83066a1f1108f35cfea7b0f93ae24bb70d6e55c8eeff82b4f9a86dae4496f77b";
const ORIGIN: &str = "https://cbgpu.com";
type FutureOutput<'a> = Pin<Box<dyn Future<Output = Result<Output, ()>> + Send + 'a>>;
struct Command {
    args: Vec<String>,
    input: Option<Vec<u8>>,
    capture: bool,
}
impl std::fmt::Debug for Command {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("WslCommand(private input redacted)")
    }
}
struct Output {
    code: i32,
    bytes: Vec<u8>,
}
trait Runner: Send + Sync {
    fn execute<'a>(&'a self, command: Command) -> FutureOutput<'a>;
}
struct RealRunner;
impl Runner for RealRunner {
    fn execute<'a>(&'a self, command: Command) -> FutureOutput<'a> {
        Box::pin(async move {
            #[cfg(not(target_os = "windows"))]
            {
                let Command {
                    args,
                    input,
                    capture,
                } = command;
                drop((args, input, capture));
                Err(())
            }
            #[cfg(target_os = "windows")]
            {
                // Resolve only the OS installation's fixed WSL executable, never a PATH-selected helper.
                let mut child = tokio::process::Command::new(wsl_executable()?)
                    .args(command.args)
                    .env_remove("WSLENV")
                    .stdin(if command.input.is_some() {
                        Stdio::piped()
                    } else {
                        Stdio::null()
                    })
                    .stdout(if command.capture {
                        Stdio::piped()
                    } else {
                        Stdio::null()
                    })
                    .stderr(Stdio::null())
                    .kill_on_drop(true)
                    .spawn()
                    .map_err(|_| ())?;
                let input = command.input;
                let writer = if let Some(bytes) = input {
                    let mut stdin = child.stdin.take().ok_or(())?;
                    Some(tokio::spawn(async move {
                        stdin.write_all(&bytes).await.map_err(|_| ())?;
                        stdin.shutdown().await.map_err(|_| ())
                    }))
                } else {
                    None
                };
                tokio::time::timeout(Duration::from_secs(90), async {
                    let mut bytes = Vec::new();
                    if let Some(stdout) = child.stdout.take() {
                        stdout
                            .take((MAX_OUTPUT + 1) as u64)
                            .read_to_end(&mut bytes)
                            .await
                            .map_err(|_| ())?;
                        if bytes.len() > MAX_OUTPUT {
                            return Err(());
                        }
                    }
                    if let Some(writer) = writer {
                        writer.await.map_err(|_| ())??;
                    }
                    let status = child.wait().await.map_err(|_| ())?;
                    Ok(Output {
                        code: status.code().unwrap_or(-1),
                        bytes,
                    })
                })
                .await
                .map_err(|_| ())?
            }
        })
    }
}
#[cfg(target_os = "windows")]
fn wsl_executable() -> Result<std::path::PathBuf, ()> {
    system_program("wsl.exe").map_err(|_| ())
}
/// Resolve the three reviewed Windows system helpers without a PATH or working-directory lookup.
#[cfg(target_os = "windows")]
pub fn system_program(name: &str) -> std::io::Result<std::path::PathBuf> {
    use std::os::windows::ffi::OsStringExt;
    if !matches!(name, "wsl.exe" | "icacls.exe" | "rundll32.exe") {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "Unsupported system helper",
        ));
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetWindowsDirectoryW(buffer: *mut u16, length: u32) -> u32;
    }
    let mut buffer = [0u16; 32768];
    // The API writes at most the supplied buffer length; the result is checked before slicing.
    let length = unsafe { GetWindowsDirectoryW(buffer.as_mut_ptr(), buffer.len() as u32) } as usize;
    if length == 0 || length >= buffer.len() {
        return Err(std::io::Error::other(
            "Cannot resolve Windows system directory",
        ));
    }
    let root = std::path::PathBuf::from(std::ffi::OsString::from_wide(&buffer[..length]));
    if !root.is_absolute() {
        return Err(std::io::Error::other("Invalid Windows system directory"));
    }
    Ok(root.join("System32").join(name))
}
fn command(
    distro: &str,
    program: &str,
    args: &[&str],
    input: Option<&[u8]>,
    capture: bool,
) -> Command {
    let mut fixed = vec![
        "--distribution".into(),
        distro.into(),
        "--exec".into(),
        program.into(),
    ];
    fixed.extend(args.iter().map(|v| (*v).into()));
    Command {
        args: fixed,
        input: input.map(Vec::from),
        capture,
    }
}
fn user_command(distro: &str, uid: u32, program: &str, args: &[&str], capture: bool) -> Command {
    let runtime = format!("XDG_RUNTIME_DIR=/run/user/{uid}");
    let bus = format!("DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/{uid}/bus");
    let mut env_args = vec![runtime.as_str(), bus.as_str(), program];
    env_args.extend_from_slice(args);
    command(distro, "/usr/bin/env", &env_args, None, capture)
}
async fn run(r: &dyn Runner, c: Command) -> Result<Vec<u8>, ()> {
    let out = r.execute(c).await?;
    if out.code != 0 || out.bytes.len() > MAX_OUTPUT {
        return Err(());
    }
    Ok(out.bytes)
}
fn text(bytes: &[u8]) -> Result<String, ()> {
    if bytes.len() > MAX_OUTPUT {
        return Err(());
    }
    let text = if bytes.contains(&0) {
        if !bytes.len().is_multiple_of(2) {
            return Err(());
        }
        String::from_utf16(
            &bytes
                .as_chunks::<2>()
                .0
                .iter()
                .map(|v| u16::from_le_bytes([v[0], v[1]]))
                .collect::<Vec<_>>(),
        )
        .map_err(|_| ())?
    } else {
        String::from_utf8(bytes.to_vec()).map_err(|_| ())?
    };
    Ok(text.trim_start_matches('\u{feff}').trim().to_owned())
}
fn distro_name(name: &str) -> bool {
    matches!(name, "Ubuntu" | "Ubuntu-22.04" | "Ubuntu-24.04")
}
fn choose_distro(bytes: &[u8]) -> Result<String, ()> {
    let rows = text(bytes)?;
    let mut candidates = Vec::new();
    for row in rows.lines() {
        let fields = row
            .trim()
            .trim_start_matches('*')
            .split_whitespace()
            .collect::<Vec<_>>();
        if fields.len() >= 3 && distro_name(fields[0]) && fields.last() == Some(&"2") {
            candidates.push(fields[0].to_owned());
        }
    }
    // Never guess among multiple distributions. The user must leave one explicitly supported target.
    if candidates.len() != 1 {
        return Err(());
    }
    Ok(candidates.remove(0))
}
fn valid_home(s: &str) -> bool {
    s.strip_prefix("/home/").is_some_and(|v| {
        !v.is_empty()
            && v.len() <= 64
            && v.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
            && v != "."
            && v != ".."
    })
}
fn uuid(s: &str) -> bool {
    s.len() == 36
        && s.bytes().enumerate().all(|(i, b)| {
            if [8, 13, 18, 23].contains(&i) {
                b == b'-'
            } else {
                b.is_ascii_hexdigit()
            }
        })
}
fn hex64(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn gpu_count(bytes: &[u8]) -> Result<usize, ()> {
    let value = text(bytes)?;
    let mut ids = std::collections::HashSet::new();
    for line in value.lines() {
        let Some((id, driver)) = line.split_once(',') else {
            return Err(());
        };
        let id = id.trim();
        let driver = driver.trim();
        if id.len() != 40
            || !id.starts_with("GPU-")
            || id[4..].bytes().enumerate().any(|(i, b)| {
                if [8, 13, 18, 23].contains(&i) {
                    b != b'-'
                } else {
                    !b.is_ascii_hexdigit()
                }
            })
            || driver.is_empty()
            || !driver.bytes().all(|b| b.is_ascii_digit() || b == b'.')
            || !ids.insert(id.to_ascii_lowercase())
        {
            return Err(());
        }
    }
    if !(1..=16).contains(&ids.len()) {
        return Err(());
    }
    Ok(ids.len())
}
struct Probe {
    distro: String,
    home: String,
    uid: u32,
    systemd: bool,
    user_systemd: bool,
    linger: bool,
    docker: bool,
    toolkit: bool,
    gpus: usize,
}
impl Probe {
    fn ready(&self) -> bool {
        self.systemd
            && self.user_systemd
            && self.linger
            && self.docker
            && self.toolkit
            && self.gpus > 0
    }
}
async fn probe(r: &dyn Runner) -> Result<Probe, ()> {
    let distro = choose_distro(
        &run(
            r,
            Command {
                args: vec!["--list".into(), "--verbose".into()],
                input: None,
                capture: true,
            },
        )
        .await?,
    )?;
    let check = |p: &str, a: &[&str]| command(&distro, p, a, None, true);
    let os = text(&run(r, check("/usr/bin/cat", &["/etc/os-release"])).await?)?;
    if !os.lines().any(|s| s == "ID=ubuntu" || s == "ID=\"ubuntu\"") {
        return Err(());
    }
    if text(&run(r, check("/usr/bin/uname", &["-m"])).await?)? != "x86_64" {
        return Err(());
    }
    let uid = text(&run(r, check("/usr/bin/id", &["-u"])).await?)?
        .parse::<u32>()
        .map_err(|_| ())?;
    if uid == 0 {
        return Err(());
    }
    let home = text(&run(r, check("/usr/bin/printenv", &["HOME"])).await?)?;
    if !valid_home(&home) {
        return Err(());
    }
    if text(&run(r, check("/usr/bin/stat", &["-c", "%u", "--", &home])).await?)?
        .parse::<u32>()
        .map_err(|_| ())?
        != uid
    {
        return Err(());
    }
    let systemd = run(r, check("/usr/bin/ps", &["-p", "1", "-o", "comm="]))
        .await
        .and_then(|b| text(&b))
        .is_ok_and(|v| v == "systemd");
    let user_systemd = run(
        r,
        user_command(
            &distro,
            uid,
            "/usr/bin/systemctl",
            &["--user", "is-active", "default.target"],
            true,
        ),
    )
    .await
    .and_then(|b| text(&b))
    .is_ok_and(|v| v == "active");
    let uid_text = uid.to_string();
    let linger = run(
        r,
        check(
            "/usr/bin/loginctl",
            &["show-user", &uid_text, "-p", "Linger", "--value"],
        ),
    )
    .await
    .and_then(|b| text(&b))
    .is_ok_and(|v| v == "yes");
    let docker = run(
        r,
        check(
            "/usr/bin/docker",
            &["version", "--format", "{{.Server.Os}}/{{.Server.Arch}}"],
        ),
    )
    .await
    .and_then(|b| text(&b))
    .is_ok_and(|v| v == "linux/amd64");
    let toolkit = run(
        r,
        check(
            "/usr/bin/docker",
            &["info", "--format", "{{json .Runtimes}}"],
        ),
    )
    .await
    .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).map_err(|_| ()))
    .is_ok_and(|v| v.get("nvidia").is_some_and(|v| v.is_object()));
    let mut gpus = 0;
    // WSL injects its driver utility here without installing a Linux NVIDIA driver.
    for binary in ["/usr/bin/nvidia-smi", "/usr/lib/wsl/lib/nvidia-smi"] {
        if let Ok(count) = run(
            r,
            check(
                binary,
                &[
                    "--query-gpu=uuid,driver_version",
                    "--format=csv,noheader,nounits",
                ],
            ),
        )
        .await
        .and_then(|b| gpu_count(&b))
        {
            gpus = count;
            break;
        }
    }
    Ok(Probe {
        distro,
        home,
        uid,
        systemd,
        user_systemd,
        linger,
        docker,
        toolkit,
        gpus,
    })
}
/// Read-only detection. WSL presence never means execution is ready.
pub(crate) async fn diagnose() -> Result<serde_json::Value, ()> {
    diagnose_with(&RealRunner).await
}
async fn diagnose_with(r: &dyn Runner) -> Result<serde_json::Value, ()> {
    let p = probe(r).await?;
    Ok(
        serde_json::json!({"wsl_ubuntu2_detected":true,"ubuntu":true,"architecture":"AMD64","user_systemd_ready":p.systemd&&p.user_systemd,"user_lingering":p.linger,"existing_nvidia_driver_ready":p.gpus>0,"docker_linux_ready":p.docker,"nvidia_docker_runtime_detected":p.toolkit,"prerequisites_ready":p.ready(),"windows_install_validated":VALIDATED,"windows_boot_persistence_proven":false,"execution_ready":false,"drivers_changed":false,"support":"EXPERIMENTAL_WSL2_UBUNTU_ONLY"}),
    )
}
fn validate_inputs(
    agent: &[u8],
    sha: &str,
    enrollment: &[u8],
    policy: Option<&[u8]>,
) -> Result<(), ()> {
    if agent.len() < 64
        || agent.len() > MAX_AGENT
        || &agent[..4] != b"\x7fELF"
        || agent[4] != 2
        || agent[5] != 1
        || agent[18] != 62
        || agent[19] != 0
        || !hex64(sha)
        || hex::encode(Sha256::digest(agent)) != sha
    {
        return Err(());
    }
    if enrollment.len() > 16384 {
        return Err(());
    }
    let e: serde_json::Value = serde_json::from_slice(enrollment).map_err(|_| ())?;
    if !e["token"]
        .as_str()
        .is_some_and(|t| t.strip_prefix("gpuenr_").is_some_and(hex64))
        || !e["node_id"].as_str().is_some_and(uuid)
        || !e["school_id"]
            .as_str()
            .is_some_and(|v| !v.is_empty() && v.len() <= 128 && !v.chars().any(char::is_control))
    {
        return Err(());
    }
    if chrono::DateTime::parse_from_rfc3339(e["expires_at"].as_str().ok_or(())?)
        .map_err(|_| ())?
        .timestamp()
        <= chrono::Utc::now().timestamp()
    {
        return Err(());
    }
    if let Some(bytes) = policy {
        if bytes.len() > 16384 {
            return Err(());
        }
        let p: serde_json::Value = serde_json::from_slice(bytes).map_err(|_| ())?;
        if p["protocol"] != 3
            || p["enabled"] != true
            || p["image_digest"] != IMAGE
            || !p["server_public_key"].as_str().is_some_and(hex64)
            || p.as_object().is_none_or(|o| {
                o.keys().any(|k| {
                    !["protocol", "enabled", "image_digest", "server_public_key"]
                        .contains(&k.as_str())
                })
            })
        {
            return Err(());
        }
    }
    Ok(())
}
async fn put(r: &dyn Runner, distro: &str, file: &str, bytes: &[u8]) -> Result<(), ()> {
    run(
        r,
        command(
            distro,
            "/usr/bin/install",
            &["-m", "0600", "--", "/dev/null", file],
            None,
            false,
        ),
    )
    .await?;
    run(
        r,
        command(distro, "/usr/bin/tee", &["--", file], Some(bytes), false),
    )
    .await?;
    Ok(())
}
/// Functional source adapter; GUI must additionally require VALIDATED before enabling this action.
pub(crate) async fn install(
    agent_bytes: &[u8],
    agent_sha256: &str,
    enrollment_bytes: &[u8],
    policy_bytes: Option<&[u8]>,
) -> Result<(), ()> {
    install_with(
        &RealRunner,
        agent_bytes,
        agent_sha256,
        enrollment_bytes,
        policy_bytes,
    )
    .await
}
async fn install_with(
    r: &dyn Runner,
    agent: &[u8],
    sha: &str,
    enrollment: &[u8],
    policy: Option<&[u8]>,
) -> Result<(), ()> {
    validate_inputs(agent, sha, enrollment, policy)?;
    let p = probe(r).await?;
    if !p.ready() {
        return Err(());
    }
    let folder = format!(
        "{}/.cbgpu-node-setup-{}",
        p.home,
        hex::encode(rand::random::<[u8; 16]>())
    );
    // mkdir is atomic and refuses an existing leaf. Never chmod an existing directory.
    run(
        r,
        command(
            &p.distro,
            "/usr/bin/mkdir",
            &["-m", "0700", "--", &folder],
            None,
            false,
        ),
    )
    .await?;
    if text(
        &run(
            r,
            command(
                &p.distro,
                "/usr/bin/stat",
                &["-c", "%u:%a", "--", &folder],
                None,
                true,
            ),
        )
        .await?,
    )? != format!("{}:700", p.uid)
    {
        return Err(());
    }
    let binary = format!("{folder}/cbgpu-node");
    let token = format!("{folder}/enrollment.json");
    let identity = format!("{folder}/identity.json");
    let policy_file = format!("{folder}/notebook-policy.json");
    put(r, &p.distro, &binary, agent).await?;
    let checked = text(
        &run(
            r,
            command(
                &p.distro,
                "/usr/bin/sha256sum",
                &["--", &binary],
                None,
                true,
            ),
        )
        .await?,
    )?;
    if checked.split_whitespace().next() != Some(sha) {
        return Err(());
    }
    run(
        r,
        command(
            &p.distro,
            "/usr/bin/chmod",
            &["0500", "--", &binary],
            None,
            false,
        ),
    )
    .await?;
    put(r, &p.distro, &token, enrollment).await?;
    if let Some(bytes) = policy {
        put(r, &p.distro, &policy_file, bytes).await?;
    }
    // Recheck expiry immediately before redeeming. Input secrets never appear in argv or stdout.
    validate_inputs(agent, sha, enrollment, policy)?;
    let initialized = run(
        r,
        user_command(
            &p.distro,
            p.uid,
            &binary,
            &["init", ORIGIN, &token, &identity],
            false,
        ),
    )
    .await;
    let removed = run(
        r,
        command(&p.distro, "/usr/bin/rm", &["--", &token], None, false),
    )
    .await;
    initialized?;
    removed?;
    let args = if policy.is_some() {
        vec![
            "install-executor-user-service",
            identity.as_str(),
            policy_file.as_str(),
            "--apply",
        ]
    } else {
        vec!["install-user-service", identity.as_str(), "--apply"]
    };
    run(r, user_command(&p.distro, p.uid, &binary, &args, false)).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    fn actual(c: &Command) -> Vec<String> {
        if c.args.get(3).is_some_and(|v| v == "/usr/bin/env") {
            c.args[..3]
                .iter()
                .chain(c.args[6..].iter())
                .cloned()
                .collect()
        } else {
            c.args.clone()
        }
    }
    struct Fake {
        calls: Mutex<Vec<Command>>,
        sha: String,
        listing: String,
        uid: u32,
        docker: bool,
        wrong_hash: bool,
        fail_init: bool,
        wsl_nvidia_only: bool,
        unsupported_gpu: bool,
        user_bus_missing: bool,
    }
    impl Fake {
        fn new(sha: String) -> Self {
            Self {
                calls: Mutex::new(Vec::new()),
                sha,
                listing: "  NAME STATE VERSION\n* Ubuntu-24.04 Running 2\n".into(),
                uid: 1000,
                docker: true,
                wrong_hash: false,
                fail_init: false,
                wsl_nvidia_only: false,
                unsupported_gpu: false,
                user_bus_missing: false,
            }
        }
    }
    impl Runner for Fake {
        fn execute<'a>(&'a self, c: Command) -> FutureOutput<'a> {
            Box::pin(async move {
                if c.args.get(3).is_some_and(|v| v == "/usr/bin/env") {
                    assert_eq!(c.args[4], format!("XDG_RUNTIME_DIR=/run/user/{}", self.uid));
                    assert_eq!(
                        c.args[5],
                        format!(
                            "DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/{}/bus",
                            self.uid
                        )
                    );
                }
                let normalized = actual(&c);
                let program = normalized.get(3).map(String::as_str).unwrap_or("");
                let a = &normalized;
                let mut code = 0;
                let bytes = if a == &["--list", "--verbose"] {
                    self.listing
                        .encode_utf16()
                        .flat_map(u16::to_le_bytes)
                        .collect()
                } else {
                    match program {
                        "/usr/bin/cat" => b"ID=ubuntu\n".to_vec(),
                        "/usr/bin/uname" => b"x86_64\n".to_vec(),
                        "/usr/bin/id" => self.uid.to_string().into_bytes(),
                        "/usr/bin/printenv" => b"/home/school\n".to_vec(),
                        "/usr/bin/stat" => {
                            if a.get(5).is_some_and(|v| v == "%u:%a") {
                                format!("{}:700\n", self.uid).into_bytes()
                            } else {
                                self.uid.to_string().into_bytes()
                            }
                        }
                        "/usr/bin/ps" => b"systemd\n".to_vec(),
                        "/usr/bin/systemctl" => {
                            if self.user_bus_missing {
                                code = 1;
                                Vec::new()
                            } else {
                                b"active\n".to_vec()
                            }
                        }
                        "/usr/bin/loginctl" => b"yes\n".to_vec(),
                        "/usr/bin/docker" => {
                            if !self.docker {
                                code = 1;
                                Vec::new()
                            } else if a.get(4).is_some_and(|v| v == "version") {
                                b"linux/amd64\n".to_vec()
                            } else {
                                br#"{"nvidia":{"path":"nvidia-container-runtime"}}"#.to_vec()
                            }
                        }
                        "/usr/bin/nvidia-smi" | "/usr/lib/wsl/lib/nvidia-smi" => {
                            if self.wsl_nvidia_only && program == "/usr/bin/nvidia-smi" {
                                code = 127;
                                Vec::new()
                            } else if self.unsupported_gpu {
                                b"Not Supported, N/A\n".to_vec()
                            } else {
                                b"GPU-aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa, 550.54\n".to_vec()
                            }
                        }
                        "/usr/bin/sha256sum" => format!(
                            "{}  {}\n",
                            if self.wrong_hash {
                                "0".repeat(64)
                            } else {
                                self.sha.clone()
                            },
                            a.last().unwrap()
                        )
                        .into_bytes(),
                        _ => {
                            if program.ends_with("/cbgpu-node")
                                && a.get(4).is_some_and(|v| v == "init")
                                && self.fail_init
                            {
                                code = 1
                            }
                            Vec::new()
                        }
                    }
                };
                self.calls.lock().unwrap().push(c);
                Ok(Output { code, bytes })
            })
        }
    }
    fn inputs() -> (Vec<u8>, String, Vec<u8>, Vec<u8>) {
        let mut elf = vec![0; 128];
        elf[..4].copy_from_slice(b"\x7fELF");
        elf[4] = 2;
        elf[5] = 1;
        elf[18] = 62;
        let sha = hex::encode(Sha256::digest(&elf));
        let enrollment=serde_json::json!({"node_id":"12345678-1234-4234-8234-123456789abc","school_id":"school-fixture","token":format!("gpuenr_{}","a".repeat(64)),"sharing_enabled":false,"expires_at":(chrono::Utc::now()+chrono::Duration::minutes(9)).to_rfc3339()}).to_string().into_bytes();
        let policy=serde_json::json!({"protocol":3,"enabled":true,"server_public_key":"b".repeat(64),"image_digest":IMAGE}).to_string().into_bytes();
        (elf, sha, enrollment, policy)
    }
    #[tokio::test]
    async fn wsl_presence_is_not_execution_or_windows_validation() {
        let (_, sha, _, _) = inputs();
        let f = Fake::new(sha);
        let p = diagnose_with(&f).await.unwrap();
        assert_eq!(p["prerequisites_ready"], true);
        assert_eq!(p["execution_ready"], false);
        assert_eq!(p["windows_install_validated"], false);
        assert_eq!(p["windows_boot_persistence_proven"], false);
        assert!(f.calls.lock().unwrap().iter().all(|c| c.input.is_none()));
    }
    #[tokio::test]
    async fn installation_streams_secrets_only_and_rehashes_inside_wsl() {
        let (agent, sha, enrollment, policy) = inputs();
        let f = Fake::new(sha.clone());
        install_with(&f, &agent, &sha, &enrollment, Some(&policy))
            .await
            .unwrap();
        let calls = f.calls.lock().unwrap();
        let token = serde_json::from_slice::<serde_json::Value>(&enrollment).unwrap()["token"]
            .as_str()
            .unwrap()
            .to_owned();
        for c in calls.iter() {
            assert!(!c.args.iter().any(|a| a.contains(&token)));
            if c.args.len() > 2 {
                assert_eq!(&c.args[..3], ["--distribution", "Ubuntu-24.04", "--exec"]);
            }
            assert!(!c.args.iter().any(|a| {
                ["/bin/sh", "/bin/bash", "sudo", "su", "powershell.exe"].contains(&a.as_str())
            }));
            if c.input.is_some() {
                assert_eq!(c.args[3], "/usr/bin/tee");
                assert!(!c.capture);
                assert!(!format!("{c:?}").contains(&token));
            }
        }
        assert_eq!(calls.iter().filter(|c| c.input.is_some()).count(), 3);
        let hash_at = calls
            .iter()
            .position(|c| actual(c).get(3).is_some_and(|a| a == "/usr/bin/sha256sum"))
            .unwrap();
        let chmod = calls
            .iter()
            .position(|c| actual(c).get(3).is_some_and(|a| a == "/usr/bin/chmod"))
            .unwrap();
        assert!(hash_at < chmod);
        assert_eq!(calls[chmod].args[4], "0500");
        assert!(calls[chmod].args.last().unwrap().ends_with("/cbgpu-node"));
        assert!(calls.iter().any(|c| {
            actual(c)
                .get(4)
                .is_some_and(|a| a == "install-executor-user-service")
                && c.args.last().is_some_and(|a| a == "--apply")
        }));
        assert_eq!(
            calls
                .iter()
                .filter(|c| actual(c).get(3).is_some_and(|a| a == "/usr/bin/rm"))
                .count(),
            1
        );
    }
    #[tokio::test]
    async fn wrong_hash_or_missing_docker_never_redeems_enrollment() {
        let (agent, sha, enrollment, _) = inputs();
        let mut f = Fake::new(sha.clone());
        f.wrong_hash = true;
        assert!(
            install_with(&f, &agent, &sha, &enrollment, None)
                .await
                .is_err()
        );
        assert!(
            !f.calls
                .lock()
                .unwrap()
                .iter()
                .any(|c| actual(c).get(4).is_some_and(|a| a == "init"))
        );
        let mut f = Fake::new(sha.clone());
        f.docker = false;
        assert!(
            install_with(&f, &agent, &sha, &enrollment, None)
                .await
                .is_err()
        );
        assert!(
            !f.calls
                .lock()
                .unwrap()
                .iter()
                .any(|c| actual(c).get(3).is_some_and(|a| a == "/usr/bin/mkdir"))
        );
    }
    #[tokio::test]
    async fn old_wsl_multiple_targets_and_root_user_fail_closed() {
        let (_, sha, _, _) = inputs();
        for listing in [
            "Ubuntu Running 1",
            "Ubuntu Running 2\nUbuntu-24.04 Running 2",
            "Debian Running 2",
        ] {
            let mut f = Fake::new(sha.clone());
            f.listing = listing.into();
            assert!(probe(&f).await.is_err());
        }
        let mut f = Fake::new(sha);
        f.uid = 0;
        assert!(probe(&f).await.is_err());
    }
    #[tokio::test]
    async fn injected_wsl_driver_is_detected_without_changing_a_driver() {
        let (_, sha, _, _) = inputs();
        let mut f = Fake::new(sha);
        f.wsl_nvidia_only = true;
        let value = diagnose_with(&f).await.unwrap();
        assert_eq!(value["existing_nvidia_driver_ready"], true);
        assert_eq!(value["execution_ready"], false);
        let calls = f.calls.lock().unwrap();
        assert!(calls.iter().any(|c| {
            actual(c)
                .get(3)
                .is_some_and(|v| v == "/usr/lib/wsl/lib/nvidia-smi")
        }));
        assert!(calls.iter().all(|c| c.input.is_none()));
        assert!(
            calls
                .iter()
                .all(|c| !c.args.iter().any(|v| v == "apt" || v == "sudo"))
        );
    }
    #[tokio::test]
    async fn missing_user_bus_or_unsupported_gpu_blocks_every_install_write() {
        let (agent, sha, enrollment, _) = inputs();
        for missing_bus in [true, false] {
            let mut f = Fake::new(sha.clone());
            f.user_bus_missing = missing_bus;
            f.unsupported_gpu = !missing_bus;
            let value = diagnose_with(&f).await.unwrap();
            assert_eq!(value["prerequisites_ready"], false);
            assert_eq!(value["execution_ready"], false);
            assert!(
                install_with(&f, &agent, &sha, &enrollment, None)
                    .await
                    .is_err()
            );
            let calls = f.calls.lock().unwrap();
            assert!(calls.iter().all(|c| c.input.is_none()));
            assert!(calls.iter().all(|c| !actual(c).get(3).is_some_and(|v| {
                [
                    "/usr/bin/mkdir",
                    "/usr/bin/install",
                    "/usr/bin/tee",
                    "/usr/bin/chmod",
                ]
                .contains(&v.as_str())
            })));
        }
    }
    #[tokio::test]
    async fn failed_init_removes_only_new_enrollment_and_keeps_recovery_identity() {
        let (agent, sha, enrollment, _) = inputs();
        let mut f = Fake::new(sha.clone());
        f.fail_init = true;
        assert!(
            install_with(&f, &agent, &sha, &enrollment, None)
                .await
                .is_err()
        );
        let calls = f.calls.lock().unwrap();
        let removed = calls
            .iter()
            .filter(|c| actual(c).get(3).is_some_and(|a| a == "/usr/bin/rm"))
            .collect::<Vec<_>>();
        assert_eq!(removed.len(), 1);
        assert!(
            removed[0]
                .args
                .last()
                .unwrap()
                .ends_with("/enrollment.json")
        );
        assert!(!calls.iter().any(|c| c.args.iter().any(|a| a == "-rf")));
        assert!(!calls.iter().any(|c| {
            actual(c)
                .get(4)
                .is_some_and(|a| a == "install-user-service")
        }));
    }
    #[test]
    fn input_and_path_checks_are_strict() {
        let (agent, sha, enrollment, policy) = inputs();
        assert!(validate_inputs(&agent, &sha, &enrollment, Some(&policy)).is_ok());
        assert!(validate_inputs(&agent, &"0".repeat(64), &enrollment, None).is_err());
        let mut p = serde_json::from_slice::<serde_json::Value>(&policy).unwrap();
        p["image_digest"] = serde_json::json!("unreviewed/image:latest");
        assert!(
            validate_inputs(&agent, &sha, &enrollment, Some(p.to_string().as_bytes())).is_err()
        );
        for home in [
            "/root",
            "/mnt/c/Users/test",
            "/home/../root",
            "/home/u/other",
            "/home/",
        ] {
            assert!(!valid_home(home));
        }
        assert!(valid_home("/home/school"));
        assert!(!uuid("------------------------------------"));
    }
}
