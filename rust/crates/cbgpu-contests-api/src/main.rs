use cbgpu_contests_api::{
    Api,
    auth::{Auth, PgSessions},
    backend::Backend,
};
use std::{collections::BTreeSet, sync::Arc};
#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("Contest API startup failed: {error}");
        std::process::exit(1)
    }
}
async fn run() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::args().any(|v| v == "--help") {
        println!(
            "Native caller-JWT contest registration API. Required CBGPU_CONTEST_INTERNAL_KEY_FILE, CBGPU_CONTEST_AUTH_DATABASE_URL, CBGPU_CONTEST_ANON_KEY_FILE. Default127.0.0.1:3545; no migration/service-role contest writes. Explicit HTTPS browser origins in CBGPU_CONTEST_ALLOWED_ORIGINS (comma-separated). Server clients omit Origin."
        );
        return Ok(());
    }
    let address: std::net::SocketAddr = std::env::var("CBGPU_CONTEST_LISTEN")
        .unwrap_or("127.0.0.1:3545".into())
        .parse()?;
    if !address.ip().is_loopback() {
        return Err("Contest API must remain loopback-only".into());
    }
    let key = std::fs::read_to_string(std::env::var("CBGPU_CONTEST_INTERNAL_KEY_FILE")?)?
        .trim()
        .as_bytes()
        .to_vec();
    let anon = std::fs::read_to_string(std::env::var("CBGPU_CONTEST_ANON_KEY_FILE")?)?
        .trim()
        .to_owned();
    let origin = std::env::var("CBGPU_CONTEST_BACKEND_URL")
        .unwrap_or("http://127.0.0.1:54321".into())
        .trim_end_matches('/')
        .to_owned();
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(4)
        .after_connect(|connection, _| {
            Box::pin(async move {
                sqlx::query("SET default_transaction_read_only=on")
                    .execute(connection)
                    .await?;
                Ok(())
            })
        })
        .connect(&std::env::var("CBGPU_CONTEST_AUTH_DATABASE_URL")?)
        .await?;
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(10))
        .build()?;
    let identity = Arc::new(Auth {
        origin: origin.clone(),
        anon_key: anon.clone(),
        sessions: Arc::new(PgSessions { pool }),
        client: client.clone(),
        account_v2: matches!(
            std::env::var("CBGPU_ACCOUNT_V2").as_deref(),
            Ok("1" | "true")
        ),
    });
    let origins:BTreeSet<_>=std::env::var("CBGPU_CONTEST_ALLOWED_ORIGINS").unwrap_or("https://cbgpu.com,https://app.cbgpu.com,https://student.cbgpu.com,https://researcher.cbgpu.com,https://developer.cbgpu.com".into()).split(',').filter(|v|!v.trim().is_empty()).map(|v|v.trim().to_owned()).collect();
    let api = Api::new(
        identity,
        Arc::new(Backend {
            origin,
            anon_key: anon,
            client,
        }),
        key,
        origins,
    )?;
    axum::serve(
        tokio::net::TcpListener::bind(address).await?,
        cbgpu_contests_api::router(api),
    )
    .with_graceful_shutdown(async {
        #[cfg(unix)]
        {
            let mut term =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                    .expect("SIGTERM handler");
            tokio::select! {_=tokio::signal::ctrl_c()=>{},_=term.recv()=>{}}
        }
        #[cfg(not(unix))]
        {
            let _ = tokio::signal::ctrl_c().await;
        }
    })
    .await?;
    Ok(())
}
