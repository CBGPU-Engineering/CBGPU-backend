//! Credential-free container helper: supervises the existing application's
//! supported loopback command and bridges raw TCP. It knows no CBGPU identities.
use std::{net::SocketAddr, time::Duration};
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().collect();
    if args.len() == 3 && args[1] == "--health" {
        let address: SocketAddr = args[2].parse()?;
        if !address.ip().is_loopback() {
            return Err("health target must be loopback".into());
        }
        let r = reqwest::Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(3))
            .build()?
            .get(format!("http://{address}/"))
            .send()
            .await?;
        if !r.status().is_success() {
            return Err("application HTTP health failed".into());
        }
        return Ok(());
    }

    if args.len() != 3 {
        return Err("expected container listen and application loopback address".into());
    }
    let listen: SocketAddr = args[1].parse()?;
    let upstream: SocketAddr = args[2].parse()?;
    if !upstream.ip().is_loopback() {
        return Err("application must remain loopback-only".into());
    }
    let _ = std::fs::remove_file("/tmp/cbgpu-ready");
    let mut child = tokio::process::Command::new("python")
        .args([
            "-m",
            "backspace_mvp.web",
            "--port",
            &upstream.port().to_string(),
        ])
        .kill_on_drop(true)
        .spawn()?;
    let mut ready = false;
    for _ in 0..300 {
        if child.try_wait()?.is_some() {
            return Err("application exited during startup".into());
        }
        if tokio::net::TcpStream::connect(upstream).await.is_ok() {
            ready = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    if !ready {
        return Err("application did not become ready".into());
    }
    let listener = tokio::net::TcpListener::bind(listen).await?;
    std::fs::write("/tmp/cbgpu-ready", b"private-application-bridge")?;
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    let permits = std::sync::Arc::new(tokio::sync::Semaphore::new(64));
    loop {
        tokio::select! {r=child.wait()=>{let _=r?;return Err("application exited".into())},_=terminate.recv()=>{return Ok(())},_=tokio::signal::ctrl_c()=>{return Ok(())},incoming=listener.accept()=>{let(mut incoming,_)=incoming?;let Ok(permit)=permits.clone().try_acquire_owned()else{continue};tokio::spawn(async move{let _permit=permit;if let Ok(Ok(mut outgoing))=tokio::time::timeout(Duration::from_secs(5),tokio::net::TcpStream::connect(upstream)).await{let _=tokio::io::copy_bidirectional(&mut incoming,&mut outgoing).await;}});}}
    }
}
