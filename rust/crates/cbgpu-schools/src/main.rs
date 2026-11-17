use cbgpu_schools::{Config, router};
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
        enabled: matches!(
            std::env::var("CBGPU_MULTISCHOOL_V1").as_deref(),
            Ok("1" | "true")
        ),
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
