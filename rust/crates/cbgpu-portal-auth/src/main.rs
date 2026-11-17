use cbgpu_portal_auth::{
    Result,
    authority::{GoTrue, Oracle},
    core::{Core, Roster},
    crypto::Crypto,
    fail, flag,
    store::Store,
};
use serde::Deserialize;
use std::{path::PathBuf, sync::Arc};
fn private_file(name: &str) -> Result<String> {
    let path = std::env::var(name).map_err(|_| fail("PRIVATE_CONFIGURATION_REQUIRED", 503))?;
    let meta = std::fs::symlink_metadata(&path)
        .map_err(|_| fail("PRIVATE_CONFIGURATION_UNAVAILABLE", 503))?;
    if !meta.is_file() || meta.len() > 65536 {
        return Err(fail("PRIVATE_CONFIGURATION_INVALID", 503));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if meta.permissions().mode() & 0o077 != 0 {
            return Err(fail("PRIVATE_CONFIGURATION_PERMISSIONS", 503));
        }
    }
    std::fs::read_to_string(path).map_err(|_| fail("PRIVATE_CONFIGURATION_UNAVAILABLE", 503))
}
#[tokio::main]
async fn main() {
    if let Err(e) = run().await {
        eprintln!("Portal auth candidate: {}", e.code);
        std::process::exit(1)
    }
}
async fn run() -> Result<()> {
    let mode = flag(std::env::var("CBGPU_ACCOUNT_V2").ok().as_deref())?
        | flag(std::env::var("CBGPU_MULTISCHOOL_V1").ok().as_deref())?;
    if !flag(std::env::var("CBGPU_PORTAL_NATIVE_AUTH").ok().as_deref())? {
        eprintln!("Native portal auth disabled; no listener, import or migration started.");
        return Ok(());
    }
    #[derive(Deserialize)]
    struct Settings {
        #[serde(rename = "sessionSecret")]
        secret: String,
    }
    let settings = private_file("CBGPU_PORTAL_SETTINGS_FILE")?;
    let settings: Settings =
        serde_json::from_str(&settings).map_err(|_| fail("PRIVATE_CONFIGURATION_INVALID", 503))?;
    let crypto = Crypto::new(&settings.secret)?;
    let roster = Roster::File(PathBuf::from(
        std::env::var("CBGPU_PORTAL_SETTINGS_FILE")
            .map_err(|_| fail("PRIVATE_CONFIGURATION_REQUIRED", 503))?,
    ));
    roster.read()?;
    let database = private_file("CBGPU_PORTAL_STORE_DATABASE_URL_FILE")?;
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    let bootstrap = args == ["--bootstrap-store"];
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(4)
        .acquire_timeout(std::time::Duration::from_secs(3))
        .after_connect(move |c, _| {
            Box::pin(async move {
                if !bootstrap {
                    sqlx::query("SET ROLE cbgpu_portal_session_store")
                        .execute(&mut *c)
                        .await?;
                }
                sqlx::query("SET statement_timeout='5000ms'")
                    .execute(c)
                    .await?;
                Ok(())
            })
        })
        .connect(database.trim())
        .await
        .map_err(|_| fail("PORTAL_STORE_UNAVAILABLE", 503))?;
    let store = Store { pool };
    if bootstrap {
        store.bootstrap().await?;
        println!(
            "Private portal runtime store bootstrapped; no Auth data or browser session modified."
        );
        return Ok(());
    }
    if args.len() == 2 && args[0] == "--import-snapshot" {
        let report = cbgpu_portal_auth::importer::import_read_only(
            &store,
            std::path::Path::new(&args[1]),
            &crypto,
        )
        .await?;
        println!(
            "{}",
            serde_json::to_string(&report).map_err(|_| fail("IMPORT_REPORT_FAILED", 503))?
        );
        return Ok(());
    }
    if !args.is_empty() {
        return Err(fail("UNKNOWN_CANDIDATE_COMMAND", 400));
    }
    let authority = private_file("CBGPU_PORTAL_AUTH_DATABASE_URL_FILE")?;
    let oracle = Oracle::connect(authority.trim()).await?;
    // Read-only readiness: never advertise healthy auth with a missing schema/oracle.
    store.get(&"0".repeat(64)).await?;
    oracle
        .principal(uuid::Uuid::nil(), uuid::Uuid::nil())
        .await?;
    let backend =
        std::env::var("CBGPU_PORTAL_AUTH_ORIGIN").unwrap_or("http://127.0.0.1:54321/".into());
    let anon = private_file("CBGPU_PORTAL_ANON_KEY_FILE")?;
    let gotrue = GoTrue::new(
        &backend,
        anon.trim().into(),
        std::env::var("CBGPU_PORTAL_NETWORK_MODE").as_deref() == Ok("private-container"),
    )?;
    let (revoked, _) = tokio::sync::broadcast::channel(64);
    let core = Arc::new(Core {
        store,
        crypto,
        roster,
        oracle,
        gotrue,
        scoped: mode,
        revoked,
    });
    let listen: std::net::SocketAddr = std::env::var("CBGPU_PORTAL_AUTH_LISTEN")
        .unwrap_or("127.0.0.1:3551".into())
        .parse()
        .map_err(|_| fail("INVALID_LISTEN_ADDRESS", 503))?;
    if !listen.ip().is_loopback() {
        return Err(fail("LOOPBACK_REQUIRED", 503));
    }
    let listener = tokio::net::TcpListener::bind(listen)
        .await
        .map_err(|_| fail("CANDIDATE_LISTENER_UNAVAILABLE", 503))?;
    axum::serve(listener, cbgpu_portal_auth::http::router(core))
        .with_graceful_shutdown(shutdown())
        .await
        .map_err(|_| fail("CANDIDATE_SERVER_FAILED", 503))
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
