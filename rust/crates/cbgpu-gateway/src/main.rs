use cbgpu_gateway::{Gateway, router};
use std::{net::SocketAddr, sync::atomic::Ordering, time::Duration};
#[tokio::main(worker_threads = 2)]
async fn main() {
    if std::env::args().nth(1).as_deref() == Some("--build-info") {
        println!(
            "{}",
            serde_json::json!({"name":"cbgpu-gateway","source":option_env!("CBGPU_SOURCE_SHA")})
        );
        return;
    }
    if let Err(message) = serve().await {
        eprintln!("Gateway startup failed: {message}");
        std::process::exit(1);
    }
}
async fn serve() -> Result<(), Box<dyn std::error::Error>> {
    let listen: SocketAddr = std::env::var("CBGPU_GATEWAY_LISTEN")
        .unwrap_or_else(|_| "127.0.0.1:3511".into())
        .parse()?;
    let upstream: SocketAddr = std::env::var("CBGPU_GATEWAY_UPSTREAM")
        .unwrap_or_else(|_| "127.0.0.1:3510".into())
        .parse()?;
    let release = std::env::var("CBGPU_RELEASE_SHA").unwrap_or_else(|_| {
        option_env!("CBGPU_SOURCE_SHA")
            .unwrap_or("development")
            .into()
    });
    let gateway = Gateway::new(upstream, listen, &release)?;
    let upgrades = gateway.upgrades.clone();
    let listener = tokio::net::TcpListener::bind(listen).await?;
    eprintln!("Rust gateway listening on {listen}; existing upstream {upstream}");
    axum::serve(listener, router(gateway))
        .with_graceful_shutdown(shutdown())
        .await?;
    // Upgraded user connections are not owned by Axum's HTTP graceful-shutdown
    // future. Let them finish; the cutover manager must not force-stop this process.
    while upgrades.load(Ordering::Relaxed) > 0 {
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    Ok(())
}
async fn shutdown() {
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .expect("termination signal");
        tokio::select! {_=tokio::signal::ctrl_c()=>{},_=terminate.recv()=>{}}
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}
