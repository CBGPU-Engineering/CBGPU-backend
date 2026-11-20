use cbgpu_private_app_gateway::{
    Gateway,
    auth::{Auth, Book, RemoteAuthority},
};
use serde::Deserialize;
use std::{
    net::SocketAddr,
    sync::{Arc, Mutex},
    time::Duration,
};
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    listen: SocketAddr,
    upstream: SocketAddr,
    app_authority: SocketAddr,
    host: String,
    project: String,
}
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().collect();
    if args.len() != 2 {
        return Err("expected private configuration path".into());
    }
    use std::os::unix::fs::PermissionsExt;
    let m = std::fs::symlink_metadata(&args[1])?;
    if !m.is_file() || m.permissions().mode() & 0o077 != 0 {
        return Err("configuration must be a private regular file".into());
    }
    let c: Config = serde_json::from_slice(&std::fs::read(&args[1])?)?;
    if !c.listen.ip().is_loopback()
        || !c.upstream.ip().is_loopback()
        || !c.app_authority.ip().is_loopback()
        || c.project.len() != 36
        || !c
            .project
            .bytes()
            .all(|b| b.is_ascii_hexdigit() || b == b'-')
        || !c.host.ends_with(".server.cbgpu.com")
        || !c
            .host
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'.' || b == b'-')
    {
        return Err("invalid isolated gateway configuration".into());
    }
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(5))
        .build()?;
    let authority = Arc::new(RemoteAuthority {
        url: format!("http://127.0.0.1:3000/api/server/{}/open", c.project),
        client: client.clone(),
    });
    let auth = Arc::new(Auth {
        authority,
        book: Mutex::new(Book::default()),
        host: c.host,
        project: c.project,
        checks: tokio::sync::Semaphore::new(32),
    });
    let g = Arc::new(Gateway {
        auth,
        listen: c.listen.to_string(),
        upstream: c.upstream.to_string(),
        app_authority: c.app_authority.to_string(),
        client,
    });
    let listener = if std::env::var("LISTEN_FDS").as_deref() == Ok("1")
        && std::env::var("LISTEN_PID")
            .ok()
            .and_then(|v| v.parse::<u32>().ok())
            == Some(std::process::id())
    {
        use std::os::fd::FromRawFd;
        let socket = unsafe { std::net::TcpListener::from_raw_fd(3) };
        if socket.local_addr()? != c.listen {
            return Err("socket activation address mismatch".into());
        }
        socket.set_nonblocking(true)?;
        tokio::net::TcpListener::from_std(socket)?
    } else {
        tokio::net::TcpListener::bind(c.listen).await?
    };
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    axum::serve(listener, cbgpu_private_app_gateway::router(g))
        .with_graceful_shutdown(async move {
            tokio::select! {_=terminate.recv()=>{},_=tokio::signal::ctrl_c()=>{}}
        })
        .await?;
    Ok(())
}
