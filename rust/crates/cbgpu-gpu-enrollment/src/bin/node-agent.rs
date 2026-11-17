//! Ubuntu-only foreground inventory reporter. Never changes drivers, daemons or GPU jobs.
#[path = "node_agent/client.rs"]
mod diagnostic_client;
#[path = "node_agent/diagnostics.rs"]
mod diagnostics;
use cbgpu_gpu_enrollment::{GpuHealth, Health, Report, hex, unhex};
use ed25519_dalek::{Signer, SigningKey};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Identity {
    url: String,
    node_id: String,
    school_id: String,
    credential_version: i64,
    sequence: i64,
    private_key: String,
}
fn lock_identity(path: &Path) -> Result<std::fs::File, Box<dyn std::error::Error>> {
    if !path.is_absolute() {
        return Err("Use an absolute identity file".into());
    }
    let mut lock_path = path.as_os_str().to_owned();
    lock_path.push(".lock");
    let mut options = std::fs::OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let file = options.open(lock_path)?;
    fs2::FileExt::try_lock_exclusive(&file)?;
    Ok(file)
}
fn ubuntu() -> bool {
    std::env::consts::OS == "linux"
        && std::fs::read_to_string("/etc/os-release").is_ok_and(|s| {
            s.lines()
                .any(|l| matches!(l, "ID=ubuntu" | "ID=\"ubuntu\""))
        })
}
fn private_read(path: &Path) -> Result<String, Box<dyn std::error::Error>> {
    let m = std::fs::symlink_metadata(path)?;
    if !m.is_file() {
        return Err("Use a private regular credential file".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if m.permissions().mode() & 0o077 != 0 {
            return Err("Credential file requires mode600".into());
        }
    }
    Ok(std::fs::read_to_string(path)?)
}
fn write(path: &Path, identity: &Identity, new: bool) -> Result<(), Box<dyn std::error::Error>> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .ok_or("Use an absolute identity-file path")?;
    if !path.is_absolute() {
        return Err("Use an absolute identity-file path".into());
    }
    if !new {
        private_read(path)?;
    }
    let tmp = parent.join(format!(".cbgpu-node-{}.tmp", hex(&random()?)));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut f = options.open(&tmp)?;
    use std::io::Write;
    f.write_all(&serde_json::to_vec(identity)?)?;
    f.sync_all()?;
    if new {
        if let Err(e) = std::fs::hard_link(&tmp, path) {
            let _ = std::fs::remove_file(&tmp);
            return Err(e.into());
        }
    } else {
        std::fs::rename(&tmp, path)?;
    }
    let _ = std::fs::remove_file(tmp);
    Ok(())
}
fn random() -> Result<[u8; 32], Box<dyn std::error::Error>> {
    let mut b = [0; 32];
    getrandom::fill(&mut b).map_err(|_| "Secure randomness unavailable")?;
    Ok(b)
}
fn endpoint(value: &str) -> Result<String, Box<dyn std::error::Error>> {
    let u = url::Url::parse(value)?;
    if u.scheme() != "https"
        || u.host_str().is_none()
        || u.port_or_known_default() != Some(443)
        || !u.username().is_empty()
        || u.password().is_some()
        || u.query().is_some()
        || u.fragment().is_some()
        || u.path() != "/"
    {
        return Err(
            "Registry must be a bare HTTPS origin with system-trusted TLS; no insecure option"
                .into(),
        );
    }
    Ok(value.trim_end_matches('/').into())
}
fn collect() -> Result<Health, Box<dyn std::error::Error>> {
    let output = std::process::Command::new("/usr/bin/nvidia-smi")
        .args([
            "--query-gpu=index,name,memory.total,memory.used,utilization.gpu,uuid",
            "--format=csv,noheader,nounits",
        ])
        .output()?;
    if !output.status.success() || output.stdout.len() > 16000 {
        return Err("Existing NVIDIA driver inventory unavailable; nothing changed".into());
    }
    let text = std::str::from_utf8(&output.stdout)?;
    let mut gpus = vec![];
    for line in text.lines() {
        let p = line.split(',').map(str::trim).collect::<Vec<_>>();
        if p.len() != 6 {
            return Err("Unsupported NVIDIA inventory response".into());
        }
        gpus.push(GpuHealth {
            index: p[0].parse()?,
            hardware_uuid: Some(p[5].into()),
            name: p[1].into(),
            memory_total_mib: p[2].parse()?,
            memory_used_mib: p[3].parse()?,
            utilization_percent: p[4].parse()?,
        });
    }
    Ok(Health {
        agent_version: env!("CARGO_PKG_VERSION").into(),
        os: "ubuntu".into(),
        gpus,
    })
}
async fn post(
    client: &reqwest::Client,
    url: &str,
    payload: Value,
) -> Result<Value, Box<dyn std::error::Error>> {
    let r = client.post(url).json(&payload).send().await?;
    if !r.status().is_success() {
        return Err(
            "Registry rejected the request; credentials and response bodies are not logged".into(),
        );
    }
    Ok(r.json().await?)
}
async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if args.first().is_some_and(|s| s == "--help") {
        println!(
            "Ubuntu inventory only. init <https-origin> <private-token-file> <absolute-new-identity-file>; report <private-identity-file> [--diagnostics <private-server-trust-json>]; run <private-identity-file> [--diagnostics <private-server-trust-json>]. Diagnostics are opt-in DIAGNOSTICS_ONLY, not notebook-ready. No drivers, services, jobs, sharing or daemon changes."
        );
        return Ok(());
    }
    if !ubuntu() {
        return Err(
            "Supported node OS: Ubuntu Linux with working NVIDIA driver; no changes made".into(),
        );
    }
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(15))
        .build()?;
    match args.first().map(String::as_str) {
        Some("init") if args.len() == 4 => {
            let url = endpoint(&args[1])?;
            let target = Path::new(&args[3]);
            let _lock = lock_identity(target)?;
            if target.exists() {
                return Err("Identity file already exists; refusing replacement".into());
            }
            // Persist key and the expected registry identity before consuming the one-use token.
            // A lost enrollment response can then be followed by `report` without losing the key.
            let enrollment: Value = serde_json::from_str(&private_read(Path::new(&args[2]))?)?;
            let token = enrollment["token"]
                .as_str()
                .ok_or("Save the complete enrollment JSON to the private file")?;
            let key = SigningKey::from_bytes(&random()?);
            let identity = Identity {
                url: url.clone(),
                node_id: enrollment["node_id"]
                    .as_str()
                    .ok_or("Missing node identity")?
                    .into(),
                school_id: enrollment["school_id"]
                    .as_str()
                    .ok_or("Missing school identity")?
                    .into(),
                credential_version: 1,
                sequence: 0,
                private_key: hex(&key.to_bytes()),
            };
            write(target, &identity, true)?;
            let response = post(
                &client,
                &format!("{url}/v1/gpu/enroll"),
                json!({"token":token,"public_key":hex(key.verifying_key().as_bytes())}),
            )
            .await?;
            if response["node_id"].as_str() != Some(identity.node_id.as_str())
                || response["school_id"].as_str() != Some(identity.school_id.as_str())
                || response["credential_version"].as_i64() != Some(1)
            {
                return Err("Registry identity mismatch; key preserved, no report sent".into());
            }
            println!("Private node identity saved. No execution or sharing enabled.");
        }
        Some("report" | "run")
            if args.len() == 2 || (args.len() == 4 && args[2] == "--diagnostics") =>
        {
            let path = Path::new(&args[1]);
            let _lock = lock_identity(path)?;
            let mut diagnostics_client = None;
            loop {
                let mut identity: Identity = serde_json::from_str(&private_read(path)?)?;
                let url = endpoint(&identity.url)?;
                let key = SigningKey::from_bytes(
                    &unhex::<32>(&identity.private_key).ok_or("Invalid node key")?,
                );
                if args.len() == 4 {
                    if diagnostics_client.is_none() {
                        diagnostics_client = Some(diagnostic_client::Client::open(
                            path,
                            Path::new(&args[3]),
                            &url,
                            &identity.node_id,
                            identity.credential_version,
                            key.clone(),
                            client.clone(),
                        )?);
                    }
                    if !diagnostics_client.as_ref().is_some_and(|value| {
                        value.matches(&url, &identity.node_id, identity.credential_version, &key)
                    }) {
                        return Err("Diagnostic identity changed".into());
                    }
                }
                identity.sequence = identity
                    .sequence
                    .checked_add(1)
                    .ok_or("Sequence exhausted")?;
                let report = Report {
                    node_id: identity.node_id.clone(),
                    credential_version: identity.credential_version,
                    sequence: identity.sequence,
                    reported_at: SystemTime::now()
                        .duration_since(UNIX_EPOCH)?
                        .as_secs()
                        .try_into()?,
                    health: collect()?,
                };
                if !cbgpu_gpu_enrollment::validate_report(&report) {
                    return Err("Unsupported GPU inventory; no report sent".into());
                }
                let signature = hex(&key.sign(&serde_json::to_vec(&report)?).to_bytes());
                write(path, &identity, false)?;
                post(
                    &client,
                    &format!("{url}/v1/gpu/report"),
                    json!({"report":report,"signature":signature}),
                )
                .await?;
                println!("Signed inventory accepted; no execution or sharing.");
                if let Some(control) = diagnostics_client.as_mut() {
                    match control
                        .tick(std::sync::Arc::new(diagnostics::Nvidia::default()))
                        .await
                    {
                        Ok(_) => println!(
                            "Optional diagnostic control checked; sharing remains disabled."
                        ),
                        Err(_) => eprintln!(
                            "Optional diagnostic control unavailable; no authority assumed."
                        ),
                    }
                }
                if args[0] == "report" {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_secs(30)).await;
            }
        }
        _ => return Err(
            "Use --help; enrollment tokens are read from private files, never command arguments"
                .into(),
        ),
    }
    Ok(())
}
#[tokio::main]
async fn main() {
    if run().await.is_err() {
        eprintln!(
            "Node operation failed. Check private file permissions, Ubuntu/NVIDIA support, registry availability and enrollment validity; secret details suppressed."
        );
        std::process::exit(1)
    }
}
