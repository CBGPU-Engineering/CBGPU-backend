use cbgpu_gpu_gateway::public_start::{Native, prepare};
#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("Notebook preparation refused: {error}");
        std::process::exit(2);
    }
}
async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if args.is_empty() || args.iter().any(|v| v == "--help") {
        println!(
            "cbgpu-gpu-public-start GPU [--apply]\nDefault is read-only preparation plan; apply starts only an additional authenticated base-path notebook on 8889, never restarts existing kernels/containers."
        );
        return Ok(());
    }
    if args.len() > 2 || args.get(1).is_some_and(|v| v != "--apply") {
        return Err("Use GPU and optional --apply".into());
    }
    let number = args[0].parse::<u8>()?;
    let report = prepare(
        number,
        args.get(1).is_some(),
        &Native {
            docker: "/usr/bin/docker".into(),
        },
    )
    .await
    .map_err(std::io::Error::other)?;
    println!("{}", serde_json::to_string(&report)?);
    Ok(())
}
