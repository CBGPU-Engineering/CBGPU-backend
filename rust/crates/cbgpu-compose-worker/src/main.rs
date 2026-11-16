use tokio::io::AsyncReadExt;
#[tokio::main(worker_threads = 2)]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("ERROR: {error}");
        std::process::exit(1);
    }
}
async fn run() -> Result<(), String> {
    let mode = std::env::args().nth(1);
    if mode.as_deref() == Some("--build-info") {
        println!(
            "{}",
            serde_json::json!({"name":"cbgpu-compose-worker","source":option_env!("CBGPU_SOURCE_SHA")})
        );
        return Ok(());
    }
    let base = std::env::var_os("CBGPU_COMPOSE_BASE")
        .map(std::path::PathBuf::from)
        .ok_or("Compose storage is not configured")?;
    if mode.as_deref() == Some("--capture-existing") {
        let id = std::env::args()
            .nth(2)
            .ok_or("Project identifier is required")?;
        cbgpu_compose_worker::capture_existing(&id, &base).await?;
        return Ok(());
    }
    let mut bytes = Vec::new();
    tokio::io::stdin()
        .take(131073)
        .read_to_end(&mut bytes)
        .await
        .map_err(|_| "Invalid Compose input")?;
    if bytes.len() > 131072 {
        return Err("Invalid Compose input".into());
    }
    if mode.as_deref() == Some("--measure") {
        let attempt =
            serde_json::from_slice(&bytes).map_err(|_| "Invalid storage measurement input")?;
        let size = cbgpu_compose_worker::measure(&attempt, &base).await?;
        println!("{}", serde_json::json!({"bytes":size}));
        return Ok(());
    }
    if mode.is_some() {
        return Err("Unknown Compose worker mode".into());
    }
    let cfg = serde_json::from_slice(&bytes).map_err(|_| "Invalid Compose configuration")?;
    let result = cbgpu_compose_worker::deploy(&cfg, &base, None).await?;
    println!(
        "CBGPU_RESULT {}",
        serde_json::to_string(&result).map_err(|_| "Cannot encode deployment result")?
    );
    Ok(())
}
