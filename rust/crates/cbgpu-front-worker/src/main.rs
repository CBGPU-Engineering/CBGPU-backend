use cbgpu_front_worker::{Config, Redactor, execute};
use tokio::io::AsyncReadExt;
#[tokio::main(worker_threads = 2)]
async fn main() {
    if std::env::args().nth(1).as_deref() == Some("--build-info") {
        println!(
            "{}",
            serde_json::json!({"name":"cbgpu-front-worker","source":option_env!("CBGPU_SOURCE_SHA")})
        );
        return;
    }
    let mut input = Vec::new();
    if tokio::io::stdin()
        .take(131073)
        .read_to_end(&mut input)
        .await
        .is_err()
        || input.len() > 131072
    {
        eprintln!("ERROR: Invalid build input");
        std::process::exit(1);
    }
    let config: Config = match serde_json::from_slice(&input) {
        Ok(config) => config,
        Err(_) => {
            eprintln!("ERROR: Invalid build configuration");
            std::process::exit(1);
        }
    };
    let redactor = Redactor::new(&config);
    match execute(&config).await {
        Ok(result) => {
            redactor.log("Deployment passed. Promoting to production.");
            println!(
                "CBGPU_RESULT {}",
                serde_json::to_string(&result).expect("serializable deployment")
            );
        }
        Err(error) => {
            redactor.log(&format!("ERROR: {error}"));
            std::process::exit(1);
        }
    }
}
