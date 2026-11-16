use cbgpu_contest_tokens::{
    api::{Api, router},
    store::Store,
};
use cbgpu_contests_api::auth::{Auth, PgSessions};
use std::sync::Arc;
#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("Contest token candidate startup failed: {error}");
        std::process::exit(1)
    }
}
async fn run() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::args().any(|value| value == "--help") {
        println!(
            "Separate default-off scoped-token candidate. Requires explicit CBGPU_CONTEST_TOKENS_ENABLED=1, private reviewed token execution DB+read-only Auth DB pools. Never applies SQL or changes the existing JWT API."
        );
        return Ok(());
    }
    if std::env::var("CBGPU_CONTEST_TOKENS_ENABLED").as_deref() != Ok("1") {
        return Err("Scoped contest tokens require separate review and explicit enablement".into());
    }
    let address: std::net::SocketAddr = std::env::var("CBGPU_CONTEST_TOKENS_LISTEN")
        .unwrap_or("127.0.0.1:13545".into())
        .parse()?;
    if !address.ip().is_loopback() {
        return Err("Token candidate must remain loopback-only".into());
    }
    let key = std::fs::read_to_string(std::env::var("CBGPU_CONTEST_TOKENS_INTERNAL_KEY_FILE")?)?
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
    let auth_pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(3)
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
    // A separate EXECUTE-only login/pool intentionally permits the narrow token functions' writes.
    let exec_pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(4)
        .connect(&std::env::var("CBGPU_CONTEST_TOKENS_DATABASE_URL")?)
        .await?;
    let auth_login: String = sqlx::query_scalar("SELECT current_user::text")
        .fetch_one(&auth_pool)
        .await?;
    let exec_login: String = sqlx::query_scalar("SELECT current_user::text")
        .fetch_one(&exec_pool)
        .await?;
    if auth_login == exec_login {
        return Err("Auth reader and token executor must use separate dedicated logins".into());
    }
    cbgpu_contest_tokens::store::verify_executor(&exec_pool).await?;
    let identity = Arc::new(Auth {
        origin,
        anon_key: anon,
        sessions: Arc::new(PgSessions { pool: auth_pool }),
        client: reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(std::time::Duration::from_secs(10))
            .build()?,
        account_v2: matches!(
            std::env::var("CBGPU_ACCOUNT_V2").as_deref(),
            Ok("1" | "true")
        ),
    });
    let api = Api::new(identity, Arc::new(Store { pool: exec_pool }), key)?;
    axum::serve(tokio::net::TcpListener::bind(address).await?, router(api))
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
