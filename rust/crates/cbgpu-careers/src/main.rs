use cbgpu_careers::{App, import, outbox, router};
use sqlx::postgres::PgPoolOptions;
use std::{env, net::SocketAddr, path::PathBuf, sync::Arc, time::Duration};

async fn run() -> Result<(), &'static str> {
    let args: Vec<String> = env::args().skip(1).collect();
    if args == ["--build-info"] {
        println!(
            "{}",
            serde_json::json!({"name":"cbgpu-careers","source":option_env!("CBGPU_SOURCE_SHA")})
        );
        return Ok(());
    }
    if args.first().is_some_and(|arg| arg == "--legacy-list") {
        if args.len() != 2 {
            return Err("Use --legacy-list SOURCE");
        }
        let rows = import::legacy_admin_list(std::path::Path::new(&args[1])).await?;
        println!("{rows}");
        return Ok(());
    }
    let database =
        env::var("CAREERS_DATABASE_URL").map_err(|_| "CAREERS_DATABASE_URL is required")?;
    let pool = PgPoolOptions::new()
        .max_connections(4)
        .acquire_timeout(Duration::from_secs(5))
        .connect(&database)
        .await
        .map_err(|_| "Careers database connection failed")?;
    if args.first().is_some_and(|arg| arg == "--import-sqlite") {
        if args.len() != 2 || env::var("CAREERS_LEGACY_WRITER_QUIESCED").as_deref() != Ok("1") {
            return Err(
                "Import requires --import-sqlite SOURCE and CAREERS_LEGACY_WRITER_QUIESCED=1 after draining the legacy writer",
            );
        }
        let result = import::sqlite(&pool, std::path::Path::new(&args[1])).await?;
        println!(
            "{}",
            serde_json::to_string(&result).map_err(|_| "Cannot encode import result")?
        );
        return Ok(());
    }
    if !args.is_empty() && args != ["serve"] {
        let result = import::admin(&pool, &args).await?;
        println!("{}", result);
        return Ok(());
    }
    let public = PathBuf::from(
        env::var("CAREERS_PUBLIC_DIR").map_err(|_| "CAREERS_PUBLIC_DIR is required")?,
    );
    let roles_path =
        env::var("CAREERS_ROLES_PATH").map_err(|_| "CAREERS_ROLES_PATH is required")?;
    let roles = serde_json::from_slice(
        &tokio::fs::read(roles_path)
            .await
            .map_err(|_| "Cannot read roles")?,
    )
    .map_err(|_| "Invalid roles")?;
    let key_path =
        env::var("CAREERS_FORM_KEY_PATH").map_err(|_| "CAREERS_FORM_KEY_PATH is required")?;
    let key = tokio::fs::read_to_string(key_path)
        .await
        .map_err(|_| "Cannot read existing form key")?;
    let key = key.trim().as_bytes().to_vec();
    if key.len() != 64 || !key.iter().all(u8::is_ascii_hexdigit) {
        return Err("Existing form key has invalid format");
    }
    let listen: SocketAddr = env::var("CAREERS_LISTEN")
        .unwrap_or_else(|_| "127.0.0.1:3532".into())
        .parse()
        .map_err(|_| "Invalid listen address")?;
    if !listen.ip().is_loopback() || listen.port() < 1024 {
        return Err("Careers must listen on a non-system loopback port");
    }
    let origin = env::var("CAREERS_ORIGIN").unwrap_or_else(|_| "https://careers.cbgpu.com".into());
    let parsed = url::Url::parse(&origin).map_err(|_| "Invalid careers origin")?;
    if parsed.scheme() != "https"
        || parsed.host_str().is_none()
        || parsed.path() != "/"
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return Err("Invalid careers origin");
    }
    let app = App {
        pool,
        key: Arc::new(key),
        roles: Arc::new(roles),
        public,
        origin,
        release: env::var("CBGPU_RELEASE_SHA").unwrap_or_else(|_| "candidate".into()),
    };
    // Default is a non-sending candidate. Enable only after the legacy sender
    // has stopped and imported notification flags have been verified.
    let mail = match env::var("CAREERS_ENABLE_OUTBOX").as_deref() {
        Err(_) | Ok("0") => None,
        Ok("1") => {
            let path = env::var("CAREERS_MAIL_ENV")
                .map_err(|_| "CAREERS_MAIL_ENV is required when enabling mail")?;
            Some((
                outbox::load_key(std::path::Path::new(&path)).await?,
                outbox::client()?,
            ))
        }
        _ => return Err("CAREERS_ENABLE_OUTBOX must be 0 or 1"),
    };
    let listener = tokio::net::TcpListener::bind(listen)
        .await
        .map_err(|_| "Careers listen failed")?;
    let (stop, mut stopping) = tokio::sync::watch::channel(false);
    let mail_enabled = mail.is_some();
    let mail_task=mail.map(|(key,client)|{
        let app=app.clone();
        tokio::spawn(async move {
            let mut interval=tokio::time::interval(Duration::from_secs(60));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop { tokio::select! {
                _=stopping.changed()=>break,
                _=interval.tick()=>{if outbox::process(&app,&key,&client).await.is_err(){eprintln!("Recruitment maintenance pending; retrying later");}}
            } }
        })
    });
    println!("CBGPU Rust Careers listening on {listen}; mail outbox enabled={mail_enabled}");
    axum::serve(
        listener,
        router(app).into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(async {
        #[cfg(unix)]
        {
            if let Ok(mut term) =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            {
                tokio::select! { _=tokio::signal::ctrl_c()=>{}, _=term.recv()=>{} }
            } else {
                let _ = tokio::signal::ctrl_c().await;
            }
        }
        #[cfg(not(unix))]
        {
            let _ = tokio::signal::ctrl_c().await;
        }
    })
    .await
    .map_err(|_| "Careers server failed")?;
    let _ = stop.send(true);
    if let Some(task) = mail_task {
        let _ = task.await;
    }
    Ok(())
}
#[tokio::main(worker_threads = 2)]
async fn main() {
    if let Err(message) = run().await {
        eprintln!("{message}");
        std::process::exit(1);
    }
}
