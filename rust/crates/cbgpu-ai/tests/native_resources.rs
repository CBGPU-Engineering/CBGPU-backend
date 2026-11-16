use cbgpu_ai::{
    resources::{Card, NativeResources, Resources},
    runner::{NativeRunner, Runner},
};
use cbgpu_ai_store::{Limits, Store};
use serde_json::json;
use std::path::PathBuf;
fn tools() -> tempfile::TempDir {
    let temp = tempfile::tempdir().unwrap();
    let output = temp.path().join("driver");
    let source =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/resource_driver.rs");
    let rustc = std::env::var_os("RUSTC")
        .map(PathBuf::from)
        .unwrap_or("rustc".into());
    assert!(
        std::process::Command::new(rustc)
            .arg(source)
            .arg("-o")
            .arg(&output)
            .status()
            .unwrap()
            .success()
    );
    for name in ["docker", "nvidia-smi", "prlimit", "native-runner"] {
        std::fs::copy(&output, temp.path().join(name)).unwrap();
    }
    temp
}
#[tokio::test]
async fn native_resource_queries_are_read_only_and_parse_quoted_gpu_names() {
    let t = tools();
    let resources = NativeResources {
        database_container: "synthetic-never-used".into(),
        database_role: "readonly".into(),
        tool_dir: Some(t.path().to_owned()),
    };
    let snapshot = resources.snapshot().await.unwrap();
    assert_eq!(snapshot.cards[0].name, "Test, Card");
    assert!(snapshot.cards[0].available);
    let calls = std::fs::read_to_string(t.path().join("calls")).unwrap();
    assert!(!calls.contains("--gpu-reset") && !calls.contains(" stop "));
}
#[tokio::test]
async fn reconciliation_and_runner_touch_only_owned_labelled_native_children() {
    let t = tools();
    let mut runner =
        NativeRunner::new(t.path().join("native-runner"), "synthetic-image".into()).unwrap();
    runner.tool_dir = Some(t.path().to_owned());
    runner.native_library_path = Some(t.path().to_string_lossy().into_owned());
    runner.recover_owned().await.unwrap();
    let calls = std::fs::read_to_string(t.path().join("calls")).unwrap();
    assert!(calls.contains("rm -f cbgpu-ai-10000000-0000-4000-8000-000000000001"));
    assert!(!calls.contains("rm -f notebook-user"));
    std::fs::write(t.path().join("wrong-label"), "").unwrap();
    std::fs::write(t.path().join("calls"), "").unwrap();
    runner.recover_owned().await.unwrap();
    assert!(
        !std::fs::read_to_string(t.path().join("calls"))
            .unwrap()
            .contains("rm -f")
    );
    std::fs::remove_file(t.path().join("wrong-label")).unwrap();
    let state = tempfile::tempdir().unwrap();
    let pool = sqlx::postgres::PgPoolOptions::new()
        .connect_lazy("postgresql://unused@127.0.0.1:1/never_used")
        .unwrap();
    let store = Store::new(pool, state.path().to_owned(), None, Limits::default()).unwrap();
    let id = "10000000-0000-4000-8000-000000000001";
    std::fs::create_dir_all(store.root.join("jobs").join(id)).unwrap();
    let job = json!({"id":id,"kind":"chat","input":{}});
    let gpu = Card {
        index: 0,
        uuid: "GPU-one".into(),
        name: "Synthetic".into(),
        memory: 0,
        utilization: 0,
        reserved: false,
        available: true,
    };
    let mut task = runner.start(&store, &job, &gpu).await.unwrap();
    assert!(!task.own_pids().await.unwrap().is_empty());
    let mut exited = false;
    for _ in 0..100 {
        if task.poll().await.unwrap().is_some() {
            exited = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert!(exited);
    task.stop().await.unwrap();
    assert!(
        store
            .root
            .join("jobs")
            .join(id)
            .join("result.json")
            .is_file()
    );
}
#[test]
fn script_wrapper_is_not_a_native_inference_runner() {
    let t = tempfile::tempdir().unwrap();
    let path = t.path().join("runner.py");
    std::fs::write(&path, "#!/usr/bin/python3\nprint('proxy')\n").unwrap();
    assert!(NativeRunner::new(path, "unused".into()).is_err());
}
