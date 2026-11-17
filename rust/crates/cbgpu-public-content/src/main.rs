use cbgpu_public_content::{Config, router};
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let address: std::net::SocketAddr = std::env::var("CBGPU_PUBLIC_LISTEN")
        .unwrap_or("127.0.0.1:3547".into())
        .parse()?;
    if !address.ip().is_loopback()
        && std::env::var("CBGPU_PUBLIC_NETWORK_MODE").as_deref() != Ok("private-container")
    {
        return Err("Non-loopback binding requires explicit private-container network mode".into());
    }
    // Optional, separately scoped login. Absence preserves anonymous JSON reads
    // without emitting aliases that the service cannot actually resolve.
    let assets = if let Ok(file) = std::env::var("CBGPU_PUBLIC_ASSET_DATABASE_URL_FILE") {
        let metadata = std::fs::symlink_metadata(&file)
            .map_err(|_| "Public asset credential file unavailable")?;
        if !metadata.is_file() || metadata.len() > 16384 {
            return Err(
                "Public asset credential file must be a bounded regular private file".into(),
            );
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if metadata.permissions().mode() & 0o077 != 0 {
                return Err(
                    "Public asset credential file permissions must be 600 or stricter".into(),
                );
            }
        }
        let database = std::fs::read_to_string(file)
            .map_err(|_| "Public asset credential file unavailable")?;
        Some(
            cbgpu_public_content::assets::Assets::connect(
                database.trim(),
                &std::env::var("CBGPU_PUBLIC_STORAGE_URL")
                    .unwrap_or("http://127.0.0.1:54321".into()),
                std::env::var("CBGPU_PUBLIC_NETWORK_MODE").ok().as_deref(),
            )
            .await?,
        )
    } else {
        None
    };
    let config = Config {
        backend_url: std::env::var("CBGPU_PUBLIC_BACKEND_URL")
            .unwrap_or("http://127.0.0.1:54321".into())
            .trim_end_matches('/')
            .into(),
        anon_key: std::env::var("CBGPU_PUBLIC_ANON_KEY")?,
        assets,
        client: reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(std::time::Duration::from_secs(15))
            .build()?,
    };
    axum::serve(
        tokio::net::TcpListener::bind(address).await?,
        router(config),
    )
    .with_graceful_shutdown(shutdown())
    .await?;
    Ok(())
}
async fn shutdown() {
    #[cfg(unix)]
    {
        let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("SIGTERM handler");
        tokio::select! {_=tokio::signal::ctrl_c()=>{},_=term.recv()=>{}}
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}
