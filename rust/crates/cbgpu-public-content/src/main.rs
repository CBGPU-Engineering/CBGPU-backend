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
    let config = Config {
        backend_url: std::env::var("CBGPU_PUBLIC_BACKEND_URL")
            .unwrap_or("http://127.0.0.1:54321".into())
            .trim_end_matches('/')
            .into(),
        anon_key: std::env::var("CBGPU_PUBLIC_ANON_KEY")?,
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
