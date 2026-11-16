use cbgpu_core::{AppState, Settings, pool, private_file, router, run};
use serde_json::json;
use std::sync::Arc;
#[tokio::main(worker_threads = 2)]
async fn main() {
    if let Err(message) = entry().await {
        eprintln!("{}", json!({"ok":false,"message":message}));
        std::process::exit(1);
    }
}
async fn entry() -> Result<(), &'static str> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("--help") {
        println!(
            "cbgpu-core [serve|migrate|study-auto|timetable-refresh] [--dry-run]\nPrivate Axum service and PostgreSQL-backed operational workers."
        );
        return Ok(());
    }
    let command = args.first().map(String::as_str).unwrap_or("serve");
    if !["serve", "migrate", "study-auto", "timetable-refresh"].contains(&command)
        || args.iter().skip(1).any(|s| s != "--dry-run")
    {
        return Err("Unsupported command or option");
    }
    let settings = Settings::from_env()?;
    if command == "migrate" {
        let db = pool(&settings.database_url, 1)
            .await
            .map_err(|_| "Migration database unavailable")?;
        sqlx::migrate!("../../migrations")
            .run(&db)
            .await
            .map_err(|_| "Schema migration failed; inspect database administratively")?;
        db.close().await;
        println!("{}", json!({"ok":true,"schema":"operations"}));
        return Ok(());
    }
    if command != "serve" {
        println!(
            "{}",
            run(
                &settings,
                command,
                chrono::Utc::now(),
                args.iter().any(|s| s == "--dry-run")
            )
            .await?
        );
        return Ok(());
    }
    let db = pool(&settings.database_url, 4)
        .await
        .map_err(|_| "Runtime database unavailable")?;
    let state = AppState {
        pool: db.clone(),
        key: Arc::from(private_file(&settings.admin_key_file).await?),
        release: Arc::from(settings.release),
    };
    let listener = tokio::net::TcpListener::bind(settings.listen)
        .await
        .map_err(|_| "Cannot bind private Rust service")?;
    eprintln!("Rust core listening on {}", settings.listen);
    axum::serve(listener, router(state))
        .with_graceful_shutdown(shutdown())
        .await
        .map_err(|_| "Rust core listener failed")?;
    db.close().await;
    Ok(())
}
async fn shutdown() {
    #[cfg(unix)]
    {
        let mut signal = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("termination signal");
        tokio::select! {_=tokio::signal::ctrl_c()=>{},_=signal.recv()=>{}}
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}
