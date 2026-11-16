use cbgpu_accounts::{Config, router};
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let addr: std::net::SocketAddr = std::env::var("CBGPU_ACCOUNTS_BIND")
        .unwrap_or("127.0.0.1:3544".into())
        .parse()?;
    if !addr.ip().is_loopback() {
        return Err("Accounts candidate must bind to loopback".into());
    }
    let backend_url = std::env::var("CBGPU_ACCOUNTS_BACKEND_URL")
        .unwrap_or("http://127.0.0.1:54321".into())
        .trim_end_matches('/')
        .to_owned();
    let anon_key = std::env::var("CBGPU_ACCOUNTS_ANON_KEY")?;
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(20))
        .build()?;
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(
        listener,
        router(Config {
            backend_url,
            anon_key,
            client,
        }),
    )
    .with_graceful_shutdown(async {
        let _ = tokio::signal::ctrl_c().await;
    })
    .await?;
    Ok(())
}
