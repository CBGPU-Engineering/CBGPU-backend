//! Host operator only. Fixed, reversible loopback routing; never a public API.
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::{io::AsyncWriteExt, process::Command};
const NETCTL: &str = "cbgpu/rust-netctl:2";
const GATEWAY_UID: &str = "65532";
async fn tool(program: &str, args: &[&str], input: Option<&[u8]>) -> Result<String, String> {
    let mut command = Command::new(program);
    command
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    if input.is_some() {
        command.stdin(Stdio::piped());
    }
    let mut child = command
        .spawn()
        .map_err(|_| format!("Cannot start {program}"))?;
    if let Some(input) = input {
        child
            .stdin
            .take()
            .ok_or("Missing tool input")?
            .write_all(input)
            .await
            .map_err(|_| "Cannot write tool input")?;
    }
    let output = tokio::time::timeout(Duration::from_secs(180), child.wait_with_output())
        .await
        .map_err(|_| "Operator tool timed out")?
        .map_err(|_| "Operator tool failed")?;
    if !output.status.success() {
        return Err(format!(
            "{program} failed: {}",
            String::from_utf8_lossy(&output.stderr)
                .chars()
                .take(1200)
                .collect::<String>()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}
async fn docker(args: &[&str]) -> Result<String, String> {
    tool("docker", args, None).await
}
async fn iptables(args: &[&str]) -> Result<String, String> {
    let mut command = vec![
        "run",
        "--rm",
        "--network",
        "host",
        "--cap-drop",
        "ALL",
        "--cap-add",
        "NET_ADMIN",
        "--security-opt",
        "no-new-privileges",
        "--read-only",
        NETCTL,
        "-w",
        "5",
    ];
    command.extend_from_slice(args);
    docker(&command).await
}
fn validate_ports(source: u16, target: u16) -> Result<(), String> {
    if source < 1024 || target < 1024 || source == target {
        return Err("Use distinct non-system loopback ports".into());
    }
    Ok(())
}
async fn prepare_netctl() -> Result<(), String> {
    if docker(&["image", "inspect", NETCTL]).await.is_ok() {
        return Ok(());
    }
    let file =
        b"FROM alpine:3.22\nRUN apk add --no-cache iptables\nENTRYPOINT [\"/usr/sbin/iptables\"]\n";
    tool("docker", &["build", "-t", NETCTL, "-"], Some(file)).await?;
    Ok(())
}
fn chain(test: bool) -> &'static str {
    if test {
        "CBGPU_RUST_TEST"
    } else {
        "CBGPU_RUST_DEPLOY"
    }
}
fn comment(test: bool) -> &'static str {
    if test {
        "cbgpu-rust-test-v1"
    } else {
        "cbgpu-rust-deploy-v1"
    }
}
async fn install_route(source: u16, target: u16, test: bool) -> Result<(), String> {
    validate_ports(source, target)?;
    let source = source.to_string();
    let target = target.to_string();
    let name = chain(test);
    let marker = comment(test);
    if iptables(&["-t", "nat", "-S", name]).await.is_ok() {
        return Err("Owned cutover chain already exists; refusing to overwrite a route".into());
    }
    iptables(&["-t", "nat", "-N", name]).await?;
    let result = async {
        iptables(&[
            "-t",
            "nat",
            "-A",
            name,
            "-m",
            "owner",
            "--uid-owner",
            GATEWAY_UID,
            "-j",
            "RETURN",
        ])
        .await?;
        iptables(&[
            "-t",
            "nat",
            "-A",
            name,
            "-p",
            "tcp",
            "-d",
            "127.0.0.1/32",
            "--dport",
            &source,
            "-j",
            "REDIRECT",
            "--to-ports",
            &target,
        ])
        .await?;
        // Chain construction affects nothing until this one atomic insertion. Existing
        // conntrack entries keep their original destination and are never flushed.
        iptables(&[
            "-t",
            "nat",
            "-I",
            "OUTPUT",
            "1",
            "-p",
            "tcp",
            "-d",
            "127.0.0.1/32",
            "--dport",
            &source,
            "-m",
            "comment",
            "--comment",
            marker,
            "-j",
            name,
        ])
        .await?;
        Ok::<(), String>(())
    }
    .await;
    if result.is_err() {
        let _ = remove_route(source.parse().expect("u16 source"), test).await;
    }
    result
}
async fn remove_route(source: u16, test: bool) -> Result<(), String> {
    let source = source.to_string();
    let name = chain(test);
    let marker = comment(test);
    if let Ok(contents) = iptables(&["-t", "nat", "-S", name]).await {
        let expected_target = if test { "18511" } else { "3511" };
        for line in contents.lines().filter(|line| line.starts_with("-A ")) {
            let owner =
                line.contains(&format!("--uid-owner {GATEWAY_UID}")) && line.ends_with("-j RETURN");
            let redirect = line.contains("-d 127.0.0.1/32")
                && line.contains(&format!("--dport {source} "))
                && line.contains(&format!("-j REDIRECT --to-ports {expected_target}"));
            if !owner && !redirect {
                return Err("Refusing to remove a routing chain with unexpected contents".into());
            }
        }
    }
    let exists = iptables(&[
        "-t",
        "nat",
        "-C",
        "OUTPUT",
        "-p",
        "tcp",
        "-d",
        "127.0.0.1/32",
        "--dport",
        &source,
        "-m",
        "comment",
        "--comment",
        marker,
        "-j",
        name,
    ])
    .await
    .is_ok();
    if exists {
        iptables(&[
            "-t",
            "nat",
            "-D",
            "OUTPUT",
            "-p",
            "tcp",
            "-d",
            "127.0.0.1/32",
            "--dport",
            &source,
            "-m",
            "comment",
            "--comment",
            marker,
            "-j",
            name,
        ])
        .await?;
    }
    if iptables(&["-t", "nat", "-S", name]).await.is_ok() {
        iptables(&["-t", "nat", "-F", name]).await?;
        iptables(&["-t", "nat", "-X", name]).await?;
    }
    Ok(())
}
async fn build_gateway(binary: &Path, tag: &str) -> Result<(), String> {
    if !binary.is_file() {
        return Err("Static Rust gateway binary is unavailable".into());
    }
    let directory =
        std::env::temp_dir().join(format!("cbgpu-gateway-image-{}", std::process::id()));
    std::fs::create_dir(&directory).map_err(|_| "Cannot create private image context")?;
    let result=async{
  std::fs::copy(binary,directory.join("cbgpu-gateway")).map_err(|_|"Cannot stage gateway binary")?;
  std::fs::write(directory.join("Dockerfile"),"FROM scratch\nCOPY --chmod=0555 cbgpu-gateway /cbgpu-gateway\nUSER 65532:65532\nENTRYPOINT [\"/cbgpu-gateway\"]\n").map_err(|_|"Cannot write image recipe")?;
  docker(&["build","--network","none","-t",tag,directory.to_str().ok_or("Invalid build context path")?]).await?;Ok::<(),String>(())
 }.await;
    let _ = std::fs::remove_dir_all(&directory);
    result
}
async fn start_gateway(
    tag: &str,
    name: &str,
    source: u16,
    target: u16,
    release: &str,
) -> Result<(), String> {
    let listen = format!("CBGPU_GATEWAY_LISTEN=127.0.0.1:{target}");
    let upstream = format!("CBGPU_GATEWAY_UPSTREAM=127.0.0.1:{source}");
    let release = format!("CBGPU_RELEASE_SHA={release}");
    docker(&[
        "run",
        "-d",
        "--name",
        name,
        "--network",
        "host",
        "--cap-drop",
        "ALL",
        "--security-opt",
        "no-new-privileges",
        "--read-only",
        "--pids-limit",
        "64",
        "--memory",
        "128m",
        "--cpus",
        "1",
        "--restart",
        "unless-stopped",
        "--log-opt",
        "max-size=5m",
        "--log-opt",
        "max-file=2",
        "--env",
        &listen,
        "--env",
        &upstream,
        "--env",
        &release,
        tag,
    ])
    .await?;
    let expected = "65532:65532";
    if docker(&["inspect", "--format", "{{.Config.User}}", name]).await? != expected {
        return Err("Gateway does not run under the dedicated routing UID".into());
    }
    for _ in 0..40 {
        if gateway_health(target).await.is_ok() {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Err("New gateway failed readiness".into())
}
async fn gateway_health(port: u16) -> Result<Value, String> {
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(2))
        .build()
        .map_err(|_| "Probe client failed")?;
    let response = client
        .get(format!("http://127.0.0.1:{port}/__cbgpu_gateway/health"))
        .send()
        .await
        .map_err(|_| "Gateway probe failed")?;
    let value: Value = response
        .json()
        .await
        .map_err(|_| "Gateway probe was not JSON")?;
    if value["status"] != "ok" || value["implementation"] != "rust-axum-gateway" {
        return Err("Unexpected gateway identity".into());
    }
    Ok(value)
}
async fn request(client: &reqwest::Client, port: u16) -> Result<String, String> {
    client
        .get(format!("http://127.0.0.1:{port}/probe"))
        .send()
        .await
        .map_err(|_| "Route test connection failed")?
        .text()
        .await
        .map_err(|_| "Route test response failed".to_owned())
}
async fn route_test(binary: &Path) -> Result<(), String> {
    let source = 18510;
    let target = 18511;
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, source))
        .await
        .map_err(|_| "Route test source port is in use")?;
    let target_probe = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, target))
        .await
        .map_err(|_| "Route test target port is in use")?;
    drop(target_probe);
    let server = tokio::spawn(async move {
        let _ = axum::serve(
            listener,
            axum::Router::new().fallback(|| async { "route-test-backend" }),
        )
        .await;
    });
    prepare_netctl().await?;
    let name = format!("cbgpu-rust-gateway-test-{}", std::process::id());
    let tag = "cbgpu/rust-gateway:route-test";
    let mut installed = false;
    let result = async {
        build_gateway(binary, tag).await?;
        start_gateway(tag, &name, source, target, "route-test").await?;
        let old = reqwest::Client::builder()
            .no_proxy()
            .build()
            .map_err(|_| "Test client failed")?;
        if request(&old, source).await? != "route-test-backend" {
            return Err("Existing connection probe failed".into());
        }
        install_route(source, target, true).await?;
        installed = true;
        if request(&old, source).await? != "route-test-backend" {
            return Err("Old connection did not survive route activation".into());
        }
        let new = reqwest::Client::builder()
            .no_proxy()
            .build()
            .map_err(|_| "New client failed")?;
        if request(&new, source).await? != "route-test-backend" {
            return Err("New connection route failed".into());
        }
        let total = gateway_health(target).await?["requests_total"]
            .as_u64()
            .unwrap_or(0);
        if total != 1 {
            return Err("New connections did not use the Rust route exactly once".into());
        }
        remove_route(source, true).await?;
        if request(&new, source).await? != "route-test-backend" {
            return Err("A connection created on Rust did not survive rollback".into());
        }
        let direct = reqwest::Client::builder()
            .no_proxy()
            .build()
            .map_err(|_| "Rollback client failed")?;
        if request(&direct, source).await? != "route-test-backend" {
            return Err("New connections did not return to the prior listener".into());
        }
        if gateway_health(target).await?["requests_total"].as_u64() != Some(2) {
            return Err("Rollback routing counters did not match expected preserved flows".into());
        }
        drop(direct);
        drop(old);
        drop(new);
        Ok::<(), String>(())
    }
    .await;
    let cleanup = if installed {
        remove_route(source, true).await
    } else {
        Ok(())
    };
    let _ = docker(&["rm", "-f", &name]).await;
    server.abort();
    result?;
    cleanup?;
    println!(
        "{}",
        json!({"ok":true,"isolated_ports":[source,target],"old_connection_preserved":true,"new_connection_on_rust":true,"rollback_preserves_existing_connections":true})
    );
    Ok(())
}
async fn activate(binary: &Path) -> Result<(), String> {
    let release =
        std::env::var("CBGPU_RELEASE_SHA").map_err(|_| "Explicit release identity is required")?;
    if release.len() != 40 || !release.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("Invalid release identity".into());
    }
    let main = tool(
        "gh",
        &[
            "api",
            "repos/CBGPU-Engineering/CBGPU-backend/git/ref/heads/main",
            "--jq",
            ".object.sha",
        ],
        None,
    )
    .await?;
    if main != release {
        return Err("Only the current backend main commit may change production routing".into());
    }
    let route = format!(
        "repos/CBGPU-Engineering/CBGPU-backend/actions/workflows/ci.yml/runs?branch=main&event=push&head_sha={release}&per_page=20"
    );
    let runs: Value = serde_json::from_str(&tool("gh", &["api", &route], None).await?)
        .map_err(|_| "Invalid CI response")?;
    if !runs["workflow_runs"].as_array().is_some_and(|runs| {
        runs.iter().any(|r| {
            r["head_sha"] == release
                && r["event"] == "push"
                && r["head_branch"] == "main"
                && r["conclusion"] == "success"
        })
    }) {
        return Err("Backend main CI has not passed for this exact commit".into());
    }
    let info: Value = serde_json::from_str(
        &tool(
            binary.to_str().ok_or("Invalid binary path")?,
            &["--build-info"],
            None,
        )
        .await?,
    )
    .map_err(|_| "Gateway binary has no build identity")?;
    if info["source"] != release || info["name"] != "cbgpu-gateway" {
        return Err("Gateway binary was not compiled for the authorized commit".into());
    }
    let source = 3510;
    let target = 3511;
    let state_directory = PathBuf::from(std::env::var("HOME").map_err(|_| "HOME missing")?)
        .join(".local/share/cbgpu-rust/network");
    std::fs::create_dir_all(&state_directory).map_err(|_| "Cannot create route state directory")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&state_directory, std::fs::Permissions::from_mode(0o700))
            .map_err(|_| "Cannot protect route state directory")?;
    }
    let config = state_directory.join("deploy-route.json");
    if config.exists() {
        return Err("A deployment route state already exists; inspect before changing it".into());
    }
    let legacy = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(5))
        .build()
        .map_err(|_| "Legacy probe client failed")?
        .get("http://127.0.0.1:3510/health")
        .send()
        .await
        .map_err(|_| "Existing deployment service is unavailable")?;
    if !legacy.status().is_success() {
        return Err("Existing deployment service is not healthy".into());
    }
    prepare_netctl().await?;
    // This test touches only two reserved test ports and always removes its own
    // chain. Never flush Docker chains or restore a stale whole-host firewall dump.
    route_test(binary).await?;

    let tag = format!("cbgpu/rust-gateway:{}", &release[..release.len().min(12)]);
    let name = "cbgpu-rust-deploy-gateway";
    build_gateway(binary, &tag).await?;
    start_gateway(&tag, name, source, target, &release).await?;
    let result = install_route(source, target, false).await;
    if result.is_err() {
        let _ = docker(&["rm", "-f", name]).await;
        return result;
    }
    let check = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(5))
        .build()
        .map_err(|_| "Route probe failed")?
        .get("http://127.0.0.1:3510/health")
        .send()
        .await;
    if !check.is_ok_and(|r| r.status().is_success()) {
        remove_route(source, false).await?;
        return Err(
            "Activation probe failed; new connections were restored to the original service".into(),
        );
    }
    let value = json!({"source":source,"gateway":target,"container":name,"image":tag,"release":release,"legacy_upstream":source,"exempt_uid":GATEWAY_UID,"business_logic":"legacy until separately converted","rollback":"cbgpu-cutover rollback-deploy"});
    if std::fs::write(&config, value.to_string()).is_err() {
        remove_route(source, false).await?;
        return Err("Route metadata could not be saved; new connections were restored to the original service".into());
    }
    println!("{}", value);
    Ok(())
}
#[tokio::main(worker_threads = 2)]
async fn main() {
    let home = match std::env::var("HOME") {
        Ok(home) => home,
        Err(_) => {
            eprintln!("HOME missing");
            std::process::exit(1)
        }
    };
    let state = PathBuf::from(home).join(".local/share/cbgpu-rust/network");
    if std::fs::create_dir_all(&state).is_err() {
        eprintln!("Cannot create operator state directory");
        std::process::exit(1);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if std::fs::set_permissions(&state, std::fs::Permissions::from_mode(0o700)).is_err() {
            eprintln!("Cannot protect operator state directory");
            std::process::exit(1);
        }
    }
    let mut lock_options = std::fs::OpenOptions::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        lock_options.mode(0o600);
    }
    let lock = match lock_options
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(state.join("operator.lock"))
    {
        Ok(lock) => lock,
        Err(_) => {
            eprintln!("Cannot open operator lock");
            std::process::exit(1)
        }
    };
    if fs2::FileExt::try_lock_exclusive(&lock).is_err() {
        eprintln!("Another Rust cutover operation is running");
        std::process::exit(1);
    }
    let args: Vec<_> = std::env::args().skip(1).collect();
    let result=match args.first().map(String::as_str){
  Some("test-route")if args.len()==2=>route_test(Path::new(&args[1])).await,
  Some("activate-deploy")if args.len()==2=>activate(Path::new(&args[1])).await,
  Some("rollback-deploy")=>{
   let result=remove_route(3510,false).await;
   if result.is_ok(){if let Ok(home)=std::env::var("HOME"){let path=PathBuf::from(home).join(".local/share/cbgpu-rust/network/deploy-route.json");if path.exists(){let _=std::fs::rename(&path,path.with_extension("rolled-back.json"));}}println!("Routing restored. Existing Rust gateway connections were retained; the gateway container was not stopped.");}result
  },
  _=>Err("Usage: cbgpu-cutover test-route STATIC_GATEWAY_BINARY | activate-deploy STATIC_GATEWAY_BINARY | rollback-deploy".into()),
 };
    if let Err(error) = result {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn validates_only_distinct_unprivileged_ports() {
        assert!(validate_ports(3510, 3511).is_ok());
        assert!(validate_ports(3510, 3510).is_err());
        assert!(validate_ports(80, 3511).is_err());
    }
}
