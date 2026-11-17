use cbgpu_gpu_enrollment::{Config, router};
fn secret(name: &str) -> Result<String, Box<dyn std::error::Error>> {
    let path = std::env::var(name)?;
    let meta = std::fs::symlink_metadata(&path)?;
    if !meta.is_file() {
        return Err("Credential must be a regular private file".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if meta.permissions().mode() & 0o077 != 0 {
            return Err("Credential file must not be group/world readable".into());
        }
    }
    let value = std::fs::read_to_string(path)?.trim().to_owned();
    if value.is_empty() || value.len() > 8192 {
        return Err("Invalid credential file".into());
    }
    Ok(value)
}
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let listen: std::net::SocketAddr = std::env::var("CBGPU_GPU_REGISTRY_LISTEN")
        .unwrap_or("127.0.0.1:3548".into())
        .parse()?;
    if !listen.ip().is_loopback() {
        return Err("GPU registry must bind private loopback".into());
    }
    let config = Config {
        enabled: matches!(
            std::env::var("CBGPU_GPU_REGISTRY_ENABLED").as_deref(),
            Ok("1" | "true")
        ),
        backend_url: std::env::var("CBGPU_GPU_REGISTRY_BACKEND_URL")
            .unwrap_or("http://127.0.0.1:54321".into())
            .trim_end_matches('/')
            .into(),
        anon_key: secret("CBGPU_GPU_REGISTRY_ANON_KEY_FILE")?,
        service_key: secret("CBGPU_GPU_REGISTRY_SERVICE_KEY_FILE")?,
        client: reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(std::time::Duration::from_secs(15))
            .build()?,
    };
    axum::serve(tokio::net::TcpListener::bind(listen).await?, router(config))
        .with_graceful_shutdown(shutdown())
        .await?;
    Ok(())
}

async fn shutdown() {
    #[cfg(unix)]
    {
        if let Ok(mut term) =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            tokio::select! { _ = tokio::signal::ctrl_c() => {}, _ = term.recv() => {} }
            return;
        }
    }
    let _ = tokio::signal::ctrl_c().await;
}
