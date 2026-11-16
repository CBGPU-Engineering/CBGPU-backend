use cbgpu_gpu_gateway::{Config, Gateway, auth::SupabaseAuthority, router};
use std::{path::PathBuf, sync::Arc};
#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("GPU gateway startup failed: {error}");
        std::process::exit(1);
    }
}
async fn run() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::args().any(|s| s == "--help") {
        println!(
            "Private candidate GPU auth gateway. Default 127.0.0.1:13500 and disabled. Configure CBGPU_GPU_GATEWAY_ENABLED=1 only after review; AUTH_BACKEND_URL, ANON_KEY_FILE, SERVICE_KEY_FILE under CBGPU_GPU_. Never starts/stops notebook servers."
        );
        return Ok(());
    }
    let config = Config {
        enabled: matches!(
            std::env::var("CBGPU_GPU_GATEWAY_ENABLED").ok().as_deref(),
            Some("1" | "true")
        ),
        listen: std::env::var("CBGPU_GPU_LISTEN")
            .unwrap_or("127.0.0.1:13500".into())
            .parse()?,
        ..Config::default()
    };
    let read_key = |name: &str| -> Result<String, Box<dyn std::error::Error>> {
        Ok(
            std::fs::read_to_string(PathBuf::from(std::env::var(name)?))?
                .trim()
                .into(),
        )
    };
    let enabled = config.enabled;
    let authority = SupabaseAuthority {
        origin: std::env::var("CBGPU_GPU_AUTH_BACKEND_URL")
            .unwrap_or("http://127.0.0.1:54321".into())
            .trim_end_matches('/')
            .into(),
        anon_key: if enabled {
            read_key("CBGPU_GPU_ANON_KEY_FILE")?
        } else {
            String::new()
        },
        service_key: if enabled {
            read_key("CBGPU_GPU_SERVICE_KEY_FILE")?
        } else {
            String::new()
        },
        account_v2: matches!(
            std::env::var("CBGPU_ACCOUNT_V2").ok().as_deref(),
            Some("1" | "true")
        ),
        client: reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(std::time::Duration::from_secs(5))
            .build()?,
    };
    let state = Gateway::new(config.clone(), Arc::new(authority)).map_err(|e| e.message)?;
    let listener = tokio::net::TcpListener::bind(config.listen).await?;
    if enabled && std::env::var("CBGPU_GPU_UPSTREAM_PROXY").is_ok() {
        let address = layered_upstream(&std::env::var("CBGPU_GPU_UPSTREAM_PROXY")?, config.listen)?;
        for gpu in 1..=4 {
            state
                .set_upstream(gpu, Some(address))
                .await
                .map_err(|e| e.message)?;
        }
    } else if enabled {
        let discover = state.clone();
        tokio::spawn(async move {
            loop {
                for gpu in 1..=4 {
                    let name = format!("jupiterhub_gpu{gpu}");
                    let output=tokio::time::timeout(std::time::Duration::from_secs(3),tokio::process::Command::new("docker").args(["inspect","--format","{{.State.Running}} {{range .NetworkSettings.Networks}}{{.IPAddress}} {{end}}",&name]).kill_on_drop(true).output()).await;
                    let address = match output {
                        Ok(Ok(out)) if out.status.success() => {
                            let raw = String::from_utf8_lossy(&out.stdout);
                            let parts = raw.split_whitespace().collect::<Vec<_>>();
                            if parts.len() == 2 && parts[0] == "true" {
                                parts[1]
                                    .parse::<std::net::IpAddr>()
                                    .ok()
                                    .map(|ip| std::net::SocketAddr::new(ip, 8889))
                            } else {
                                None
                            }
                        }
                        _ => None,
                    };
                    let _ = discover.set_upstream(gpu, address).await;
                }
                tokio::time::sleep(std::time::Duration::from_secs(15)).await;
            }
        });
    }
    let drain = state.clone();
    axum::serve(listener, router(state))
        .with_graceful_shutdown(async move {
            #[cfg(unix)]
            {
                let mut terminate =
                    tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                        .expect("SIGTERM");
                tokio::select! {_ =tokio::signal::ctrl_c()=>{},_ =terminate.recv()=>{}}
            }
            #[cfg(not(unix))]
            {
                let _ = tokio::signal::ctrl_c().await;
            }
            drain.drain();
        })
        .await?;
    Ok(())
}

fn layered_upstream(
    value: &str,
    listen: std::net::SocketAddr,
) -> Result<std::net::SocketAddr, Box<dyn std::error::Error>> {
    let address: std::net::SocketAddr = value.parse()?;
    if !address.ip().is_loopback() || address.port() == 0 || address == listen {
        return Err("Trusted layered upstream must be a distinct numeric loopback address".into());
    }
    Ok(address)
}
#[cfg(test)]
mod tests {
    #[test]
    fn layered_proxy_is_explicit_numeric_loopback_and_cannot_recurse() {
        let listen = "127.0.0.1:13500".parse().unwrap();
        assert_eq!(
            super::layered_upstream("127.0.0.1:3500", listen)
                .unwrap()
                .port(),
            3500
        );
        for value in [
            "127.0.0.1:13500",
            "127.0.0.1:0",
            "8.8.8.8:3500",
            "172.17.0.2:3500",
            "localhost:3500",
            "http://127.0.0.1:3500",
        ] {
            assert!(super::layered_upstream(value, listen).is_err());
        }
    }
}
