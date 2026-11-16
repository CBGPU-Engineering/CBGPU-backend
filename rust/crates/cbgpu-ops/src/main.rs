use cbgpu_ops::{Context, Result};
use std::path::PathBuf;
#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("cbgpu-ops: {error}");
        std::process::exit(1);
    }
}
async fn run() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let action = args.next().unwrap_or("help".into());
    if action == "help" || action == "--help" {
        println!(
            "cbgpu-ops <provision-local|install-local|integration|rollback-local DIRECTORY> [--root RUST_DIRECTORY] [--container NAME]\nOnly explicit subcommands change local runtime infrastructure. No user deployment or application data is reset."
        );
        return Ok(());
    }
    let backup = if action == "rollback-local" {
        Some(PathBuf::from(
            args.next().ok_or("Rollback directory required")?,
        ))
    } else {
        None
    };
    let mut root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut container = "cbgpu-supabase-db".to_owned();
    while let Some(flag) = args.next() {
        let value = args.next().ok_or("Option value required")?;
        match flag.as_str() {
            "--root" => root = PathBuf::from(value),
            "--container" => container = value,
            _ => return Err("Unknown option".into()),
        }
    }
    let home = PathBuf::from(std::env::var_os("HOME").ok_or("Home directory unavailable")?);
    let context = Context {
        root: root.canonicalize()?,
        home,
        container,
        tool_dir: None,
        readiness_url: "http://127.0.0.1:3540/health/ready".into(),
    };
    let result = match action.as_str() {
        "provision-local" => context.provision().await?,
        "install-local" => context.install().await?,
        "integration" => context.integration().await?,
        "rollback-local" => {
            context
                .rollback(backup.as_deref().ok_or("Rollback directory required")?)
                .await?
        }
        _ => return Err("Unknown subcommand; use --help".into()),
    };
    println!("{}", serde_json::to_string(&result)?);
    Ok(())
}
