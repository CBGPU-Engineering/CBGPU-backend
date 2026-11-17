use std::path::PathBuf;
#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("Deployment admission fence refused: {error}");
        std::process::exit(1)
    }
}
async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut database = None;
    let mut key = None;
    let mut receipt = None;
    let mut action = "plan";
    let mut chosen = false;
    for arg in std::env::args().skip(1) {
        if let Some(value) = arg.strip_prefix("--database=") {
            if database.is_some() {
                return Err("Duplicate database option".into());
            }
            database = Some(PathBuf::from(value))
        } else if let Some(value) = arg.strip_prefix("--key=") {
            if key.is_some() {
                return Err("Duplicate key option".into());
            }
            key = Some(PathBuf::from(value))
        } else if let Some(value) = arg.strip_prefix("--receipt=") {
            if receipt.is_some() {
                return Err("Duplicate receipt option".into());
            }
            receipt = Some(PathBuf::from(value))
        } else if arg == "--apply" || arg == "--remove" || arg == "--plan" {
            if chosen {
                return Err("Choose one fence action".into());
            }
            chosen = true;
            action = if arg == "--plan" {
                "plan"
            } else if arg == "--apply" {
                "apply"
            } else {
                "remove"
            }
        } else if arg == "--help" {
            println!(
                "cbgpu-deploy-legacy-fence --database=EXISTING_ABSOLUTE_SQLITE --key=EXISTING_PRIVATE_RAW32_KEY [--receipt=NEW_PRIVATE_PATH --apply | --receipt=EXISTING_RECEIPT --remove]. Default is a read-only plan. No services/containers are stopped or started."
            );
            return Ok(());
        } else {
            return Err("Unknown fence argument".into());
        }
    }
    let database = database.ok_or("Existing SQLite path required")?;
    let key = key.ok_or("Existing key path required")?;
    let result = match action {
        "apply" => {
            cbgpu_deploy_control::legacy_fence::apply(
                &database,
                &key,
                &receipt.ok_or("New private receipt path required")?,
            )
            .await?
        }
        "remove" => {
            cbgpu_deploy_control::legacy_fence::remove(
                &database,
                &key,
                &receipt.ok_or("Existing receipt path required")?,
            )
            .await?
        }
        _ => cbgpu_deploy_control::legacy_fence::plan(&database, &key).await?,
    };
    println!("{}", serde_json::to_string(&result)?);
    Ok(())
}
