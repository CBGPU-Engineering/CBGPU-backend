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
    #[serde(default = "authority_default")]
    authority_origin: String,
}
fn authority_default() -> String {
    "http://127.0.0.1:3000".into()
}
fn authority_location(raw: &str, project: &str) -> Result<(String, String), &'static str> {
    let origin = url::Url::parse(raw).map_err(|_| "invalid authority origin")?;
    let host = origin.host_str().unwrap_or("");
    let local =
        origin.scheme() == "http" && matches!(host, "127.0.0.1" | "localhost" | "[::1]" | "::1");
    let first_party = origin.scheme() == "https"
        && matches!(host, "cbgpu.com" | "app.cbgpu.com")
        && origin.port().is_none();
    if (!local && !first_party)
        || !origin.username().is_empty()
        || origin.password().is_some()
        || origin.path() != "/"
        || origin.query().is_some()
        || origin.fragment().is_some()
    {
        return Err("invalid first-party authority origin");
    }
    Ok((
        format!("{}/api/server/{project}/open", raw.trim_end_matches('/')),
        if local {
            "cbgpu.com".into()
        } else {
            host.to_string()
        },
    ))
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
    let (authority_url, authority_host) = authority_location(&c.authority_origin, &c.project)?;
    let authority = Arc::new(RemoteAuthority {
        url: authority_url,
        host: authority_host,
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn authority_uses_only_fixed_first_party_or_loopback() {
        for origin in [
            "http://127.0.0.1:3000",
            "http://localhost:3000",
            "http://[::1]:3000",
            "https://cbgpu.com",
            "https://app.cbgpu.com",
        ] {
            assert!(authority_location(origin, "fixture").is_ok(), "{origin}")
        }
        for origin in [
            "http://cbgpu.com",
            "https://attacker.example",
            "https://cbgpu.com.attacker.example",
            "https://cbgpu.com/path",
            "https://cbgpu.com?next=x",
            "https://user:pass@cbgpu.com",
            "http://10.0.0.1:3000",
        ] {
            assert!(authority_location(origin, "fixture").is_err(), "{origin}")
        }
        assert_eq!(
            authority_location(&authority_default(), "fixture").unwrap(),
            (
                "http://127.0.0.1:3000/api/server/fixture/open".into(),
                "cbgpu.com".into()
            )
        );
    }
}
