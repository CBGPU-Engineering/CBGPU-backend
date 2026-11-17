use cbgpu_ai_store::{Limits, Store};
use std::path::PathBuf;
#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("cbgpu-ai-store: {error}");
        std::process::exit(1);
    }
}
async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let command = args.next().unwrap_or("help".into());
    if command == "help" || command == "--help" {
        println!(
            "cbgpu-ai-store migrate | import-read-only SQLITE_FILE | counts\nUses CBGPU_AI_DATABASE_URL and CBGPU_AI_STATE. Import requires an empty destination; never updates SQLite or moves existing files."
        );
        return Ok(());
    }
    let database = std::env::var("CBGPU_AI_DATABASE_URL")?;
    let root = PathBuf::from(std::env::var("CBGPU_AI_STATE")?);
    let data = std::env::var_os("CBGPU_AI_DATA_ROOT").map(PathBuf::from);
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(4)
        .connect(&database)
        .await?;
    let store = Store::new(pool, root, data, Limits::default())?;
    let output = match command.as_str() {
        "migrate" => {
            store.migrate().await?;
            serde_json::json!({"ok":true})
        }
        "import-read-only" => {
            cbgpu_ai_store::importer::import_read_only(
                &store,
                &PathBuf::from(args.next().ok_or("Source SQLite path required")?),
            )
            .await?
        }
        "counts" => {
            let mut result = serde_json::json!({});
            for table in [
                "jobs",
                "uploads",
                "ai_projects",
                "api_keys",
                "conversations",
                "chat_messages",
            ] {
                let count: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
                    "SELECT count(*) FROM ai_store.{table}"
                )))
                .fetch_one(&store.pool)
                .await?;
                result[table] = serde_json::json!(count);
            }
            result
        }
        _ => return Err("Unknown command".into()),
    };
    println!("{}", serde_json::to_string(&output)?);
    Ok(())
}
