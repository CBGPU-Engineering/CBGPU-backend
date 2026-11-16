//! Native subprocess ABI fixture: no model, GPU or production data involved.
use std::{fs, process::Command};
#[test]
fn model_worker_exec_preserves_scheduler_process_and_routes_only_supported_kinds() {
    let temp = tempfile::tempdir().unwrap();
    let state = temp.path().join("state");
    let folder = state.join("jobs/10000000-0000-4000-8000-000000000001");
    fs::create_dir_all(&folder).unwrap();
    fs::write(folder.join("input.json"), "{}").unwrap();
    let source = temp.path().join("fixture.rs");
    fs::write(&source, r#"
fn main(){let args:Vec<_>=std::env::args().collect();assert_eq!(args.len(),4);assert!(["embed","search","transcribe"].contains(&args[1].as_str()));assert_eq!(std::env::var("CUDA_VISIBLE_DEVICES").unwrap(),"-1");std::fs::write(std::path::Path::new(&args[2]).join("fixture-pid"),std::process::id().to_string()).unwrap();}
"#).unwrap();
    let executable = temp.path().join("native-fixture");
    assert!(
        Command::new(std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into()))
            .arg(&source)
            .arg("-o")
            .arg(&executable)
            .status()
            .unwrap()
            .success()
    );
    for kind in ["embed", "search", "transcribe"] {
        let mut child = Command::new(env!("CARGO_BIN_EXE_cbgpu-ai-inference"))
            .args([kind, folder.to_str().unwrap(), state.to_str().unwrap()])
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("CUDA_VISIBLE_DEVICES", "-1")
            .env("CBGPU_AI_MODELS_RUNNER", &executable)
            .spawn()
            .unwrap();
        let pid = child.id();
        assert!(child.wait().unwrap().success());
        assert_eq!(
            fs::read_to_string(folder.join("fixture-pid")).unwrap(),
            pid.to_string()
        );
    }
    assert!(
        !Command::new(env!("CARGO_BIN_EXE_cbgpu-ai-inference"))
            .args(["unknown", folder.to_str().unwrap(), state.to_str().unwrap()])
            .env_clear()
            .env("CBGPU_AI_MODELS_RUNNER", &executable)
            .status()
            .unwrap()
            .success()
    );
}
