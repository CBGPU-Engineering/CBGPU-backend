//! Native, explicit additional notebook preparation; never restarts containers or kernels.
use serde_json::{Value, json};
use std::{collections::BTreeSet, net::Ipv4Addr, path::PathBuf, process::Stdio, time::Duration};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
pub type Result<T> = std::result::Result<T, &'static str>;
#[derive(Debug)]
pub struct Output {
    pub success: bool,
    pub bytes: Vec<u8>,
}
#[async_trait::async_trait]
pub trait Control: Send + Sync {
    async fn command(&self, args: Vec<String>, input: Option<Vec<u8>>) -> Result<Output>;
    async fn listening(&self, ip: Ipv4Addr) -> bool;
}
pub struct Native {
    pub docker: PathBuf,
}
#[async_trait::async_trait]
impl Control for Native {
    async fn command(&self, args: Vec<String>, input: Option<Vec<u8>>) -> Result<Output> {
        if !self.docker.is_absolute() {
            return Err("Docker executable must be an explicit absolute path");
        }
        let mut child = tokio::process::Command::new(&self.docker)
            .args(args)
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("DOCKER_HOST", "unix:///var/run/docker.sock")
            .stdin(if input.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(|_| "Cannot inspect or prepare notebook")?;
        let operation = async {
            if let Some(input) = input {
                let mut stream = child.stdin.take().ok_or("Notebook input unavailable")?;
                stream
                    .write_all(&input)
                    .await
                    .map_err(|_| "Notebook input failed")?;
                drop(stream);
            }
            let mut bytes = Vec::new();
            child
                .stdout
                .take()
                .ok_or("Notebook output unavailable")?
                .take(65537)
                .read_to_end(&mut bytes)
                .await
                .map_err(|_| "Notebook output failed")?;
            if bytes.len() > 65536 {
                return Err("Notebook response exceeds bound");
            }
            let status = child
                .wait()
                .await
                .map_err(|_| "Notebook operation failed")?;
            Ok(Output {
                success: status.success(),
                bytes,
            })
        };
        match tokio::time::timeout(Duration::from_secs(8), operation).await {
            Ok(result) => {
                if result.is_err() {
                    let _ = child.kill().await;
                    let _ = child.wait().await;
                }
                result
            }
            Err(_) => {
                let _ = child.kill().await;
                let _ = child.wait().await;
                Err("Notebook operation timed out")
            }
        }
    }
    async fn listening(&self, ip: Ipv4Addr) -> bool {
        matches!(
            tokio::time::timeout(
                Duration::from_millis(300),
                tokio::net::TcpStream::connect((ip, 8889))
            )
            .await,
            Ok(Ok(_))
        )
    }
}
fn args(values: &[&str]) -> Vec<String> {
    values.iter().map(|v| (*v).into()).collect()
}
pub fn password(config: &str) -> Result<String> {
    if config.len() > 32768 {
        return Err("Notebook authentication configuration exceeds bound");
    }
    if let Ok(value) = serde_json::from_str::<Value>(config) {
        let hashes = [
            "/PasswordIdentityProvider/hashed_password",
            "/ServerApp/password",
            "/NotebookApp/password",
        ]
        .into_iter()
        .filter_map(|pointer| value.pointer(pointer).and_then(Value::as_str))
        .map(checked_hash)
        .collect::<Result<BTreeSet<_>>>()?;
        if hashes.len() == 1 {
            return Ok(hashes.into_iter().next().unwrap());
        }
        return Err("Notebook password configuration is missing or ambiguous");
    }
    let mut found = Vec::new();
    for line in config.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if ![
            "c.ServerApp.password",
            "c.NotebookApp.password",
            "c.PasswordIdentityProvider.hashed_password",
        ]
        .contains(&key.trim())
        {
            continue;
        }
        let value = value.trim();
        let quote = value
            .chars()
            .next()
            .filter(|q| ['\'', '"'].contains(q))
            .ok_or("Notebook password must be a literal hash")?;
        let tail = &value[1..];
        let end = tail
            .find(quote)
            .ok_or("Invalid notebook password literal")?;
        let remainder = tail[end + 1..].trim();
        if !remainder.is_empty() && !remainder.starts_with('#') {
            return Err("Executable notebook password configuration refused");
        }
        found.push(checked_hash(&tail[..end])?);
    }
    let unique: BTreeSet<_> = found.into_iter().collect();
    if unique.len() != 1 {
        return Err("Notebook password configuration is missing or ambiguous");
    }
    Ok(unique.into_iter().next().unwrap())
}
fn checked_hash(value: &str) -> Result<String> {
    if value.len() < 24
        || value.len() > 2048
        || !(value.starts_with("argon2:") || value.starts_with("sha1:"))
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"$:=,./_+-".contains(&b))
    {
        return Err("Unsupported notebook password hash");
    }
    Ok(value.into())
}
pub fn notebook_config(number: u8, hash: &str) -> Result<Value> {
    if !(1..=4).contains(&number) {
        return Err("GPU must be 1 through 4");
    }
    let hash = checked_hash(hash)?;
    Ok(
        json!({"ServerApp":{"ip":"0.0.0.0","port":8889,"port_retries":0,"open_browser":false,"allow_root":true,"root_dir":"/root","allow_remote_access":true,"trust_xheaders":true,"disable_check_xsrf":false,"cookie_options":{"secure":true,"samesite":"Lax"},"base_url":format!("/{number}/"),"password":hash,"token":""},"NotebookApp":{"password":hash,"token":""},"IdentityProvider":{"token":""},"PasswordIdentityProvider":{"hashed_password":hash}}),
    )
}
pub async fn prepare(number: u8, apply: bool, control: &dyn Control) -> Result<Value> {
    if !(1..=4).contains(&number) {
        return Err("GPU must be 1 through 4");
    }
    let container = format!("jupiterhub_gpu{number}");
    let inspect=control.command(args(&["inspect","--format",r#"{"name":{{json .Name}},"running":{{json .State.Running}},"network":{{json .HostConfig.NetworkMode}},"networks":{{json .NetworkSettings.Networks}}}"#,&container]),None).await?;
    if !inspect.success {
        return Err("Notebook container unavailable");
    }
    let value: Value = serde_json::from_slice(&inspect.bytes)
        .map_err(|_| "Invalid notebook container metadata")?;
    if value["name"] != format!("/{container}")
        || value["running"] != true
        || !matches!(value["network"].as_str(), Some("default" | "bridge"))
    {
        return Err("Unexpected notebook container identity or network");
    }
    let networks = value["networks"]
        .as_object()
        .ok_or("Notebook network unavailable")?;
    if networks.len() != 1 {
        return Err("Notebook network is ambiguous");
    }
    let ip: Ipv4Addr = networks
        .values()
        .next()
        .and_then(|n| n["IPAddress"].as_str())
        .ok_or("Notebook address unavailable")?
        .parse()
        .map_err(|_| "Invalid notebook address")?;
    if !ip.is_private() || ip.is_loopback() {
        return Err("Notebook address must be private");
    }
    let mut hash = None;
    for name in [
        "jupyter_server_config.py",
        "jupyter_server_config.json",
        "cbgpu-public-native.json",
        "cbgpu-public.json",
        "cbgpu-public.py",
    ] {
        let output = control
            .command(
                args(&[
                    "exec",
                    &container,
                    "head",
                    "-c",
                    "32769",
                    &format!("/root/.jupyter/{name}"),
                ]),
                None,
            )
            .await?;
        if output.success {
            hash = Some(password(
                std::str::from_utf8(&output.bytes)
                    .map_err(|_| "Invalid notebook authentication encoding")?,
            )?);
            break;
        }
    }
    let hash = hash.ok_or("Notebook password configuration not ready")?;
    let pid = control
        .command(
            args(&[
                "exec",
                &container,
                "head",
                "-c",
                "32",
                "/root/.jupyter/cbgpu-public.pid",
            ]),
            None,
        )
        .await?;
    if pid.success
        && let Some(pid) = std::str::from_utf8(&pid.bytes)
            .ok()
            .and_then(|s| s.trim().parse::<u32>().ok())
            .filter(|n| *n > 0)
    {
        let command = control
            .command(
                args(&[
                    "exec",
                    &container,
                    "head",
                    "-c",
                    "8192",
                    &format!("/proc/{pid}/cmdline"),
                ]),
                None,
            )
            .await?;
        if command.success
            && [
                b"/root/.jupyter/cbgpu-public.json".as_slice(),
                b"/root/.jupyter/cbgpu-public.py".as_slice(),
            ]
            .iter()
            .any(|marker| command.bytes.windows(marker.len()).any(|v| v == *marker))
        {
            return Ok(
                json!({"gpu":number,"ip":ip,"ready":control.listening(ip).await,"action":"existing","mutations_executed":false}),
            );
        }
    }
    if control.listening(ip).await {
        return Err("Unmanaged notebook listener already uses port 8889");
    }
    if !apply {
        return Ok(
            json!({"gpu":number,"ip":ip,"ready":false,"action":"would_start","mutations_executed":false}),
        );
    }
    let data = serde_json::to_vec(&notebook_config(number, &hash)?)
        .map_err(|_| "Cannot encode notebook configuration")?;
    let write=control.command(args(&["exec","-i",&container,"sh","-c","test -d /root/.jupyter && test ! -L /root/.jupyter && test ! -L /root/.jupyter/cbgpu-public-native.json && umask 077 && cat > /root/.jupyter/cbgpu-public-native.json && chmod 600 /root/.jupyter/cbgpu-public-native.json"]),Some(data)).await?;
    if !write.success {
        return Err("Cannot stage additional notebook configuration");
    }
    let launch=control.command(args(&["exec","-d",&container,"sh","-c","umask 077; echo $$ > /root/.jupyter/cbgpu-public.pid; exec jupyter notebook --config=/root/.jupyter/cbgpu-public-native.json >>/root/.jupyter/cbgpu-public.log 2>&1"]),None).await?;
    if !launch.success {
        return Err("Cannot start additional notebook server");
    }
    let mut ready = false;
    for _ in 0..10 {
        if control.listening(ip).await {
            ready = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Ok(json!({"gpu":number,"ip":ip,"ready":ready,"action":"started","mutations_executed":true}))
}
