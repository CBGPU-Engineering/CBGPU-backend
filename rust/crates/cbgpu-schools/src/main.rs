use cbgpu_schools::{Config, router};
fn enabled(value: Option<&str>) -> Result<bool, &'static str> {
    match value {
        None | Some("" | "0" | "false") => Ok(false),
        Some("1" | "true") => Ok(true),
        _ => Err("Invalid school API authorization configuration"),
    }
}
fn enabled_from_environment() -> Result<bool, &'static str> {
    match std::env::var("CBGPU_MULTISCHOOL_V1") {
        Ok(v) => enabled(Some(&v)),
        Err(std::env::VarError::NotPresent) => Ok(false),
        Err(_) => Err("Invalid school API authorization configuration"),
    }
}
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let listen: std::net::SocketAddr = std::env::var("CBGPU_SCHOOLS_LISTEN")
        .unwrap_or("127.0.0.1:3546".into())
        .parse()?;
    if !listen.ip().is_loopback() {
        return Err("School API must bind to private loopback".into());
    }
    let listener = tokio::net::TcpListener::bind(listen).await?;
    let config = Config {
        enabled: enabled_from_environment()?,
        backend_url: std::env::var("CBGPU_SCHOOLS_BACKEND_URL")
            .unwrap_or("http://127.0.0.1:54321".into())
            .trim_end_matches('/')
            .into(),
        anon_key: std::env::var("CBGPU_SCHOOLS_ANON_KEY")?,
        client: reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(std::time::Duration::from_secs(20))
            .build()?,
    };
    axum::serve(listener, router(config))
        .with_graceful_shutdown(shutdown())
        .await?;
    Ok(())
}

async fn shutdown() {
    let interrupt = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        if let Ok(mut signal) =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            signal.recv().await;
        } else {
            std::future::pending::<()>().await;
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {_=interrupt=>{},_=terminate=>{}}
}

#[cfg(test)]
mod flag_tests {
    #[test]
    fn canonical_feature_flag_only() {
        for value in [None, Some(""), Some("0"), Some("false")] {
            assert!(!super::enabled(value).unwrap());
        }
        for value in ["1", "true"] {
            assert!(super::enabled(Some(value)).unwrap());
        }
        for value in ["TRUE", "FALSE", " 0", "1 ", "yes"] {
            assert!(super::enabled(Some(value)).is_err());
        }
    }
}
