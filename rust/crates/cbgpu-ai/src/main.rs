use cbgpu_ai::{Api, Authorization, router};
use cbgpu_ai_store::{Limits, Store};
use std::{path::PathBuf, sync::Arc};
#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("Native AI startup failed: {error}");
        std::process::exit(1);
    }
}
async fn run() -> Result<(), Box<dyn std::error::Error>> {
    if let Some(argument) = std::env::args().nth(1) {
        match argument.as_str() {
            "--version" => {
                println!("cbgpu-ai {}", env!("CARGO_PKG_VERSION"));
                return Ok(());
            }
            "--help" => {
                println!(
                    "Native private AI API. Configure CBGPU_AI_DATABASE_URL, CBGPU_AI_STATE, CBGPU_AI_KEY_FILE and CBGPU_AI_BACKEND_KEY_FILE. CBGPU_AI_NATIVE_RUNNER explicitly attaches the reservation-first scheduler; omitted means API-only candidate. Default loopback port 13550."
                );
                return Ok(());
            }
            _ => return Err("Unknown option".into()),
        }
    }
    let address: std::net::SocketAddr = std::env::var("CBGPU_AI_LISTEN")
        .unwrap_or("127.0.0.1:13550".into())
        .parse()?;
    if !address.ip().is_loopback() {
        return Err("AI API must remain loopback-only".into());
    }
    let database = std::env::var("CBGPU_AI_DATABASE_URL")?;
    let root = PathBuf::from(std::env::var("CBGPU_AI_STATE")?);
    let key = std::fs::read(std::env::var("CBGPU_AI_KEY_FILE")?)?;
    let key = String::from_utf8(key)?.trim().as_bytes().to_vec();
    if key.len() < 16 {
        return Err("Internal key is too short".into());
    }
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(8)
        .connect(&database)
        .await?;
    let store = Store::new(
        pool,
        root,
        std::env::var_os("CBGPU_AI_DATA_ROOT").map(PathBuf::from),
        Limits {
            require_data_mount: std::env::var("CBGPU_AI_REQUIRE_DATA_MOUNT").ok().as_deref()
                == Some("1"),
            ..Limits::default()
        },
    )?;
    let authorization = Authorization {
        backend_url: std::env::var("CBGPU_AI_BACKEND_URL")
            .unwrap_or("http://127.0.0.1:54321".into()),
        service_key: std::fs::read_to_string(std::env::var("CBGPU_AI_BACKEND_KEY_FILE")?)?
            .trim()
            .to_owned(),
        account_v2: cbgpu_ai::scoped_authorization_from_environment()?,
        client: reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(std::time::Duration::from_secs(10))
            .build()?,
    };
    let api = Arc::new(Api::new(
        store,
        key,
        authorization,
        std::env::var("CBGPU_AI_OCR_IMAGE").unwrap_or("cbgpu-ocr:rust1-candidate".into()),
    ));
    let listener = tokio::net::TcpListener::bind(address).await?;
    let scheduler = if let Some(path) = std::env::var_os("CBGPU_AI_NATIVE_RUNNER") {
        let runner = cbgpu_ai::runner::NativeRunner::new(
            PathBuf::from(path),
            std::env::var("CBGPU_AI_JOB_IMAGE")
                .unwrap_or("pytorch/pytorch:2.7.1-cuda12.6-cudnn9-runtime".into()),
        )?;
        let resources = cbgpu_ai::resources::NativeResources {
            database_container: std::env::var("CBGPU_AI_DATABASE_CONTAINER")
                .unwrap_or("cbgpu-supabase-db".into()),
            database_role: std::env::var("CBGPU_AI_DATABASE_READ_ROLE")
                .unwrap_or("postgres".into()),
            tool_dir: None,
        };
        let scheduler = cbgpu_ai::scheduler::Scheduler::new(
            api.clone(),
            Arc::new(resources),
            Arc::new(runner),
            cbgpu_ai::scheduler::Settings {
                workers: std::env::var("CBGPU_AI_MAX_WORKERS")
                    .ok()
                    .and_then(|s| s.parse::<usize>().ok())
                    .unwrap_or(4)
                    .clamp(1, 4),
                ..Default::default()
            },
        );
        let task = scheduler.clone().start().await?;
        Some((scheduler, task))
    } else {
        None
    };
    let shutdown = scheduler.as_ref().map(|(scheduler, _)| scheduler.clone());
    let served = axum::serve(listener, router(api))
        .with_graceful_shutdown(async move {
            #[cfg(unix)]
            {
                let mut terminate =
                    tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                        .expect("SIGTERM handler");
                tokio::select! {_ =tokio::signal::ctrl_c()=>{},_ =terminate.recv()=>{}}
            }
            #[cfg(not(unix))]
            {
                let _ = tokio::signal::ctrl_c().await;
            }
            if let Some(scheduler) = shutdown {
                scheduler.close();
            }
        })
        .await;
    if let Some((scheduler, task)) = scheduler {
        scheduler.close();
        let _ = tokio::time::timeout(std::time::Duration::from_secs(35), task).await;
    }
    served?;
    Ok(())
}
