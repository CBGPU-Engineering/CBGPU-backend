use cbgpu_ai::model_assets::{PublicHub, manifest, offline_plan, prepare};
#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("Native model preparation refused: {error}");
        std::process::exit(1);
    }
}
async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if args.iter().any(|v| v == "--help") {
        println!(
            "cbgpu-ai-prepare-models MANIFEST_JSON ABSOLUTE_MODELS_ROOT [--apply]\nDefault offline plan: no network, files, model execution or GPU work. Explicit apply downloads only pinned public data, with no cached Hugging Face token, and never overwrites changed assets."
        );
        return Ok(());
    }
    if !(2..=3).contains(&args.len()) || args.get(2).is_some_and(|v| v != "--apply") {
        return Err("Use manifest path, absolute model root and optional --apply".into());
    }
    let models = manifest(&std::fs::read(&args[0])?)?;
    let root = std::path::Path::new(&args[1]);
    if !root.is_absolute() {
        return Err("Model root must be absolute".into());
    }
    let report = if args.len() == 3 {
        prepare(&models, root, &PublicHub).await?
    } else {
        offline_plan(&models, root)
    };
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}
