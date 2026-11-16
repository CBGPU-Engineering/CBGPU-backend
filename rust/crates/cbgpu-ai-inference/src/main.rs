use std::path::Path;
fn main() {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args == ["--build-info"] {
        println!(
            "{}",
            serde_json::json!({"name":"cbgpu-ai-inference","source":option_env!("CBGPU_SOURCE_SHA"),"implementation":"native-rust","text":true,"other_models_attached":std::env::var_os("CBGPU_AI_MODELS_RUNNER").is_some()})
        );
        return;
    }
    let result = if args.len() == 3 {
        cbgpu_ai_inference::worker::run(&args[0], Path::new(&args[1]), Path::new(&args[2]))
    } else {
        Err("Usage: cbgpu-ai-inference KIND JOB_FOLDER STATE_ROOT".into())
    };
    if let Err(message) = result {
        eprintln!("{message}");
        std::process::exit(1);
    }
}
