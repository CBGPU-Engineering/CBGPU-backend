#[tokio::main(worker_threads = 2)]
async fn main() {
    if std::env::args().nth(1).as_deref() == Some("--build-info") {
        println!(
            "{}",
            serde_json::json!({"name":"cbgpu-backup","source":option_env!("CBGPU_SOURCE_SHA")})
        );
        return;
    }
    let config = cbgpu_backup::Config {
        runtime: std::env::var_os("CBGPU_RUNTIME_BACKEND")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| "/home/cbgpu/CBGPU-live/backend".into()),
        helper_root: std::env::var_os("CBGPU_BACKUP_HELPER_ROOT")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| "/home/cbgpu/CBGPU-Engineering/CBGPU-backend-rust".into()),
        helper_extension: std::env::var("CBGPU_BACKUP_HELPER_EXTENSION")
            .unwrap_or_else(|_| "mts".into()),
    };
    match cbgpu_backup::backup(&config).await {
        Ok(_) => println!(
            "Backend snapshot and encrypted offsite upload completed; application and runtime PostgreSQL included."
        ),
        Err(message) => {
            eprintln!("{message}");
            std::process::exit(1);
        }
    }
}
