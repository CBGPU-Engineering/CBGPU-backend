use cbgpu_ad_revenue::{Config, router};
fn enabled(value: Option<&str>) -> Result<bool, &'static str> {
    match value {
        None | Some("" | "0" | "false") => Ok(false),
        Some("1" | "true") => Ok(true),
        _ => Err("Invalid ad reporting feature flag"),
    }
}
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::args().nth(1).as_deref() == Some("--help") {
        println!(
            "Private native official-report CSV import/analytics candidate. CBGPU_AD_REVENUE_ENABLED defaults off; CBGPU_AD_REVENUE_LISTEN defaults127.0.0.1:3549; configure backend URL/public anon key. No official console API connection or ad serving changes."
        );
        return Ok(());
    }
    let on = match std::env::var("CBGPU_AD_REVENUE_ENABLED") {
        Ok(v) => enabled(Some(&v))?,
        Err(std::env::VarError::NotPresent) => false,
        Err(_) => return Err("Invalid ad reporting flag".into()),
    };
    let listen: std::net::SocketAddr = std::env::var("CBGPU_AD_REVENUE_LISTEN")
        .unwrap_or("127.0.0.1:3549".into())
        .parse()?;
    if !listen.ip().is_loopback() {
        return Err("Report API must bind private loopback".into());
    }
    let origin =
        std::env::var("CBGPU_AD_REVENUE_BACKEND_URL").unwrap_or("http://127.0.0.1:54321".into());
    let url = reqwest::Url::parse(&origin)?;
    if !["http", "https"].contains(&url.scheme())
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err("Invalid report metadata origin".into());
    }
    let anon = if on {
        std::env::var("CBGPU_AD_REVENUE_ANON_KEY")?
    } else {
        String::new()
    };
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(15))
        .build()?;
    let app = router(Config {
        enabled: on,
        origin,
        anon,
        client,
    });
    let listener = tokio::net::TcpListener::bind(listen).await?;
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let mut signal =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                    .expect("SIGTERM");
            tokio::select! {_=tokio::signal::ctrl_c()=>{},_=signal.recv()=>{}}
        })
        .await?;
    Ok(())
}
