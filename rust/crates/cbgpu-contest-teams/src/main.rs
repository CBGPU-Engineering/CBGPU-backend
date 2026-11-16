use cbgpu_contest_teams::allocate;
use sqlx::postgres::PgPoolOptions;
use std::time::Duration;
#[tokio::main(worker_threads = 2)]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args == ["--build-info"] {
        println!("cbgpu-contest-teams {}", env!("CARGO_PKG_VERSION"));
        return;
    }
    if args.iter().any(|a| a != "--dry-run") {
        eprintln!("Usage: cbgpu-contest-teams [--dry-run|--build-info]");
        std::process::exit(2);
    }
    let result = async {
        let url = std::env::var("CBGPU_CONTEST_TEAMS_DATABASE_URL")
            .map_err(|_| "private database configuration missing")?;
        let pool = PgPoolOptions::new()
            .max_connections(2)
            .acquire_timeout(Duration::from_secs(10))
            .connect(&url)
            .await
            .map_err(|_| "database connection failed")?;
        let report = allocate(&pool, args.iter().any(|a| a == "--dry-run"))
            .await
            .map_err(|e| {
                eprintln!(
                    "allocation transaction failed; SQLSTATE={}",
                    e.as_database_error()
                        .and_then(|d| d.code())
                        .unwrap_or_default()
                );
                "allocation failed; transaction rolled back"
            })?;
        println!("{}", serde_json::to_string(&report).unwrap());
        Ok::<_, &str>(())
    }
    .await;
    if let Err(message) = result {
        eprintln!("{message}");
        std::process::exit(1);
    }
}
