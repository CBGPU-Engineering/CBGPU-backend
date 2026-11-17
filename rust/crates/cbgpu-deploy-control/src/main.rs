use cbgpu_deploy_control::{
    api::Api,
    authority::BackendAuthority,
    crypto::Cipher,
    store::Store,
    worker::{Controller, Executor, Native},
};
use std::{path::PathBuf, sync::Arc};
#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("Native deployment candidate startup failed: {error}");
        std::process::exit(1)
    }
}
async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().collect();
    if args.get(1).is_some_and(|v| v == "--inspect-legacy") && args.len() == 3 {
        println!(
            "{}",
            cbgpu_deploy_control::importer::inspect_read_only(std::path::Path::new(&args[2]))
                .await?
        );
        return Ok(());
    }
    if std::env::args().any(|value| value == "--help") {
        println!(
            "Default-off native deployment control candidate. Required CBGPU_DEPLOY_CONTROL_ENABLED=1, CBGPU_DEPLOY_DATABASE_URL, CBGPU_DEPLOY_KEY_FILE(existing raw32-byte legacy key), CBGPU_DEPLOY_BACKEND_KEY_FILE, CBGPU_DEPLOY_DATA. Optional workers require CBGPU_DEPLOY_WORKERS_ENABLED=1 plus trusted FRONT_WORKER/COMPOSE_WORKER/DOMAINS_WORKER absolute ELF paths. No automatic schema/import or live cutover. Reverse commands: --export-legacy PATH, --inspect-reverse PATH, --verify-reverse-current PATH, --import-reverse-bundle PATH. Export/import/current-verification require --confirm-stopped-writers and CBGPU_DEPLOY_WRITERS_FENCED=1; bounds --max-rows=N --max-bytes=N."
        );
        return Ok(());
    }
    if std::env::var("CBGPU_DEPLOY_CONTROL_ENABLED").as_deref() != Ok("1") {
        return Err("Native deployment control requires explicit reviewed enablement".into());
    }
    let address: std::net::SocketAddr = std::env::var("CBGPU_DEPLOY_LISTEN")
        .unwrap_or("127.0.0.1:13580".into())
        .parse()?;
    if !address.ip().is_loopback() {
        return Err("Deployment control must remain private loopback-only".into());
    }
    let data = PathBuf::from(std::env::var("CBGPU_DEPLOY_DATA")?).canonicalize()?;
    let cipher = Cipher::new(&std::fs::read(std::env::var("CBGPU_DEPLOY_KEY_FILE")?)?)?;
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(8)
        .connect(&std::env::var("CBGPU_DEPLOY_DATABASE_URL")?)
        .await?;
    sqlx::query("SELECT id FROM deploy_store.projects LIMIT 0")
        .fetch_all(&pool)
        .await?;
    let store = Store {
        pool,
        cipher,
        data: data.clone(),
        free_floor: 5 * 1024 * 1024 * 1024,
    };
    if args.get(1).is_some_and(|arg| {
        matches!(
            arg.as_str(),
            "--export-legacy"
                | "--import-reverse-bundle"
                | "--inspect-reverse"
                | "--verify-reverse-current"
        )
    }) {
        let command = &args[1];
        let source = args.get(2).ok_or("Reverse snapshot path required")?;
        let mut options = cbgpu_deploy_control::reverse::Options::default();
        let mut confirmed = false;
        for arg in &args[3..] {
            if arg == "--confirm-stopped-writers" {
                confirmed = true
            } else if let Some(value) = arg.strip_prefix("--max-rows=") {
                options.max_rows = value.parse()?
            } else if let Some(value) = arg.strip_prefix("--max-bytes=") {
                options.max_bytes = value.parse()?
            } else {
                return Err("Unknown reverse snapshot option".into());
            }
        }
        options.writers_fenced =
            confirmed && std::env::var("CBGPU_DEPLOY_WRITERS_FENCED").as_deref() == Ok("1");
        let result = if command == "--inspect-reverse" {
            cbgpu_deploy_control::reverse::inspect_bundle(
                &store,
                std::path::Path::new(source),
                &options,
            )?
        } else {
            if !options.writers_fenced {
                return Err(
                    "Explicit --confirm-stopped-writers and CBGPU_DEPLOY_WRITERS_FENCED=1 required"
                        .into(),
                );
            }
            let legacy: std::net::SocketAddr = std::env::var("CBGPU_DEPLOY_LEGACY_LISTEN")
                .unwrap_or("127.0.0.1:3510".into())
                .parse()?;
            if !legacy.ip().is_loopback() {
                return Err("Legacy listener must remain loopback-only".into());
            }
            for endpoint in [address, legacy] {
                if tokio::net::TcpStream::connect(endpoint).await.is_ok() {
                    return Err(
                        "Deployment writer listener still active; reverse operation refused".into(),
                    );
                }
            }
            let _lock = WorkerLock::acquire(&data)?;
            if command == "--verify-reverse-current" {
                cbgpu_deploy_control::reverse::verify_current_bundle(
                    &store,
                    std::path::Path::new(source),
                    &options,
                )
                .await?
            } else if command == "--export-legacy" {
                cbgpu_deploy_control::reverse::export_legacy(
                    &store,
                    std::path::Path::new(source),
                    &options,
                )
                .await?
            } else {
                cbgpu_deploy_control::reverse::import_bundle(
                    &store,
                    std::path::Path::new(source),
                    &options,
                )
                .await?
            }
        };
        println!("{}", result);
        return Ok(());
    }
    if args.get(1).is_some_and(|v| v == "--import-snapshot") && args.len() == 3 {
        if std::env::var("CBGPU_DEPLOY_WRITERS_FENCED").as_deref() != Ok("1") {
            return Err("Explicit legacy writer fence confirmation required".into());
        }
        let legacy: std::net::SocketAddr = std::env::var("CBGPU_DEPLOY_LEGACY_LISTEN")
            .unwrap_or("127.0.0.1:3510".into())
            .parse()?;
        if !legacy.ip().is_loopback() {
            return Err("Legacy listener must be explicit loopback".into());
        }
        if tokio::net::TcpStream::connect(legacy).await.is_ok() {
            return Err(
                "Legacy listener still active; import refused until its writers are fenced".into(),
            );
        }
        let _lock = WorkerLock::acquire(&data)?;
        let result = cbgpu_deploy_control::importer::import_read_only(
            &store,
            std::path::Path::new(&args[2]),
        )
        .await?;
        let fingerprint = cbgpu_deploy_control::importer::target_fingerprint(&store).await?;
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let receipt = serde_json::json!({"fingerprint":fingerprint,"import":result,"legacy_snapshot":args[2],"created_at":cbgpu_deploy_control::now()});
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(data.join(".native-import-receipt.json"))?;
        file.write_all(&serde_json::to_vec_pretty(&receipt)?)?;
        file.sync_all()?;
        println!(
            "Imported with row equality proof; private rollback receipt saved. No service started."
        );
        return Ok(());
    }
    if args.get(1).is_some_and(|v| v == "--rollback-check") && args.len() == 2 {
        if std::env::var("CBGPU_DEPLOY_WRITERS_FENCED").as_deref() != Ok("1") {
            return Err("Stop/fence native writers before rollback verification".into());
        }
        if tokio::net::TcpStream::connect(address).await.is_ok() {
            return Err("Native listener still active; rollback check refused".into());
        }
        let _lock = WorkerLock::acquire(&data)?;
        let receipt: serde_json::Value =
            serde_json::from_slice(&std::fs::read(data.join(".native-import-receipt.json"))?)?;
        if receipt["fingerprint"]
            != cbgpu_deploy_control::importer::target_fingerprint(&store).await?
        {
            return Err(
                "Native writes detected; restoring stale SQLite would lose data and is refused"
                    .into(),
            );
        }
        println!(
            "Native target unchanged since import; stop/fence native process and restore original listener/unit before rolling back. This check changes no data."
        );
        return Ok(());
    }
    if args.len() > 1 {
        return Err("Unknown deployment control command".into());
    }
    let authority = Arc::new(BackendAuthority {
        origin: std::env::var("CBGPU_DEPLOY_BACKEND_URL")
            .unwrap_or("http://127.0.0.1:54321".into())
            .trim_end_matches('/')
            .into(),
        key: std::fs::read_to_string(std::env::var("CBGPU_DEPLOY_BACKEND_KEY_FILE")?)?
            .trim()
            .into(),
        account_v2: cbgpu_deploy_control::config::resource_enforced(
            std::env::var("CBGPU_ACCOUNT_V2").ok().as_deref(),
            std::env::var("CBGPU_MULTISCHOOL_V1").ok().as_deref(),
        ),
        client: reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(std::time::Duration::from_secs(8))
            .build()?,
    });
    let api = Arc::new(Api {
        store,
        authority,
        workers_enabled: std::sync::atomic::AtomicBool::new(false),
    });
    // Public tenant traffic has its own listener and can never invoke private control routes.
    let public_listener = if let Ok(value) = std::env::var("CBGPU_DEPLOY_PUBLIC_LISTEN") {
        let address: std::net::SocketAddr = value.parse()?;
        if !address.ip().is_loopback() {
            return Err("Public ingress origin must remain loopback-only".into());
        }
        Some((tokio::net::TcpListener::bind(address).await?, address))
    } else {
        None
    };
    if let Some((listener, address)) = public_listener {
        let public = cbgpu_deploy_control::public::router(cbgpu_deploy_control::public::Public {
            api: api.clone(),
            listen: address,
        });
        tokio::spawn(async move {
            if let Err(error) = axum::serve(listener, public).await {
                eprintln!("Native deployment public origin stopped: {error}");
            }
        });
    }
    // Bind before recovery so a second process cannot mark an active controller's jobs interrupted.
    let listener = tokio::net::TcpListener::bind(address).await?;
    let mut guard = None;
    if std::env::var("CBGPU_DEPLOY_WORKERS_ENABLED").as_deref() == Ok("1") {
        guard = Some(WorkerLock::acquire(&data)?);
        let native = Arc::new(Native {
            front: PathBuf::from(std::env::var("CBGPU_DEPLOY_FRONT_WORKER")?),
            compose: PathBuf::from(std::env::var("CBGPU_DEPLOY_COMPOSE_WORKER")?),
            domains: PathBuf::from(std::env::var("CBGPU_DEPLOY_DOMAINS_WORKER")?),
            backend_root: PathBuf::from(std::env::var("CBGPU_DEPLOY_BACKEND_ROOT")?)
                .canonicalize()?,
            compose_base: PathBuf::from(std::env::var("CBGPU_DEPLOY_COMPOSE_BASE")?)
                .canonicalize()?,
            home: PathBuf::from(std::env::var_os("HOME").ok_or("Home directory unavailable")?),
            tool_dir: None,
            docker_socket: "/var/run/docker.sock".into(),
            log_proxies: Default::default(),
            last_measure: Default::default(),
        });
        native.validate()?;
        api.workers_enabled
            .store(true, std::sync::atomic::Ordering::SeqCst);
        let interrupted = api.store.recover_interrupted().await?;
        for work in interrupted {
            if work.kind == "front_build" {
                let cleanup = cbgpu_deploy_control::queue::Work {
                    kind: "front_cleanup".into(),
                    payload: serde_json::json!({"deployment_id":work.deployment}),
                    ..work
                };
                native.execute(api.clone(), &cleanup).await?;
            }
        }
        for family in [
            vec!["front_build", "front_cleanup"],
            vec!["compose_build", "compose_stop"],
            vec!["domain_ensure", "domain_remove"],
        ] {
            let controller = Controller {
                api: api.clone(),
                executor: native.clone(),
            };
            let kinds: Vec<String> = family.into_iter().map(str::to_owned).collect();
            tokio::spawn(async move {
                loop {
                    match controller.step(&kinds).await {
                        Ok(true) => {}
                        Ok(false) => tokio::time::sleep(std::time::Duration::from_secs(1)).await,
                        Err(_) => tokio::time::sleep(std::time::Duration::from_secs(2)).await,
                    }
                }
            });
        }
        let auto = Controller {
            api: api.clone(),
            executor: native.clone(),
        };
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(120)).await;
                let _ = auto.auto_poll().await;
            }
        });
        let controller = Controller {
            api: api.clone(),
            executor: native,
        };
        tokio::spawn(async move {
            loop {
                let _ = controller.reconcile().await;
                tokio::time::sleep(std::time::Duration::from_secs(10)).await;
            }
        });
    }
    let shutdown_api = api.clone();
    axum::serve(
        listener,
        cbgpu_deploy_control::public::combined(api, address),
    )
    .with_graceful_shutdown(async move {
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
        if shutdown_api
            .workers_enabled
            .load(std::sync::atomic::Ordering::SeqCst)
        {
            let _ = sqlx::query(
                "UPDATE deploy_store.work_items SET cancel_requested=true WHERE state='running'",
            )
            .execute(&shutdown_api.store.pool)
            .await;
        }
    })
    .await?;
    drop(guard);
    Ok(())
}
struct WorkerLock(std::fs::File);
impl WorkerLock {
    fn acquire(data: &std::path::Path) -> Result<Self, Box<dyn std::error::Error>> {
        use std::os::{fd::AsRawFd, unix::fs::OpenOptionsExt};
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(data.join(".native-controller.lock"))?;
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            return Err("Another native deployment worker owns this data root".into());
        }
        Ok(Self(file))
    }
}
impl Drop for WorkerLock {
    fn drop(&mut self) {
        use std::os::fd::AsRawFd;
        unsafe {
            libc::flock(self.0.as_raw_fd(), libc::LOCK_UN);
        }
    }
}
