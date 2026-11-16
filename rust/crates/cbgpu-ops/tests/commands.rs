use cbgpu_ops::Context;
use std::{
    fs,
    path::{Path, PathBuf},
};
fn fixture() -> (tempfile::TempDir, Context) {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let root = temp.path().join("rust");
    let tools = temp.path().join("tools");
    for p in [
        home.join(".config/cbgpu"),
        home.join(".cargo/bin"),
        root.join("target/release"),
        root.join("deploy"),
        root.join("migrations"),
        tools.clone(),
    ] {
        fs::create_dir_all(p).unwrap();
    }
    let driver = tools.join("driver");
    let source = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/driver.rs");
    let rustc = std::env::var_os("RUSTC")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("rustc"));
    assert!(
        std::process::Command::new(rustc)
            .arg(source)
            .arg("-o")
            .arg(&driver)
            .status()
            .unwrap()
            .success()
    );
    for name in ["docker", "systemctl", "git"] {
        fs::copy(&driver, tools.join(name)).unwrap();
    }
    fs::copy(&driver, root.join("target/release/cbgpu-core")).unwrap();
    fs::copy(&driver, home.join(".cargo/bin/cargo")).unwrap();
    for unit in cbgpu_ops::UNITS {
        fs::write(
            root.join("deploy").join(unit),
            "[Unit]\nDescription=synthetic\n[Service]\nExecStart=/synthetic\n",
        )
        .unwrap();
    }
    fs::write(
        root.join("migrations/20260101000000_rust_operations_core.sql"),
        "CREATE SCHEMA operations;",
    )
    .unwrap();
    let ctx = Context {
        root,
        home,
        container: "synthetic-only".into(),
        tool_dir: Some(tools),
        readiness_url: "http://127.0.0.1:1".into(),
    };
    (temp, ctx)
}
fn config(c: &Context) {
    fs::write(
        c.home.join(".config/cbgpu/rust-core.env"),
        "CBGPU_RUNTIME_DATABASE_URL=postgresql://runtime:synthetic@127.0.0.2/cbgpu_runtime\n",
    )
    .unwrap();
}
fn calls(path: &Path) -> String {
    fs::read_to_string(path.join("calls")).unwrap_or_default()
}
#[tokio::test]
async fn provision_parity_creates_scoped_roles_and_private_files_without_real_docker() {
    let (_t, c) = fixture();
    let result = c.provision().await.unwrap();
    assert_eq!(result["existing_application_tables_changed"], false);
    let log = calls(c.tool_dir.as_ref().unwrap());
    assert!(
        log.contains("create-roles")
            && log.contains("create-db")
            && log.contains("disable-owner")
            && log.contains("revoke-owner")
    );
    let env = fs::read_to_string(c.home.join(".config/cbgpu/rust-core.env")).unwrap();
    assert!(env.contains("postgresql://cbgpu_runtime:") && !env.contains("cbgpu_runtime_owner"));
    assert!(
        !c.home
            .join(".config/cbgpu/rust-core-provision-recovery.json")
            .exists()
    );
    let corelog = calls(&c.root.join("target/release"));
    assert!(corelog.contains("core migrate"));
}
#[tokio::test]
async fn integration_uses_test_database_and_always_revokes_temporary_owner() {
    let (_t, c) = fixture();
    config(&c);
    assert_eq!(c.integration().await.unwrap()["ok"], true);
    let log = calls(c.tool_dir.as_ref().unwrap());
    assert!(
        log.contains("create-db")
            && log.contains("migrate-test")
            && log.contains("drop-test-db")
            && log.contains("revoke-owner")
    );
    assert!(
        calls(&c.home.join(".cargo/bin")).contains("-p cbgpu-core --test postgres -- --ignored")
    );
}
#[tokio::test]
async fn integration_collision_never_drops_an_existing_database() {
    let (_t, c) = fixture();
    config(&c);
    fs::write(c.tool_dir.as_ref().unwrap().join("fail-create-db"), "").unwrap();
    assert!(c.integration().await.is_err());
    let log = calls(c.tool_dir.as_ref().unwrap());
    assert!(log.contains("revoke-owner"));
    assert!(!log.contains("drop-test-db"));
}
#[tokio::test]
async fn install_and_rollback_preserve_timers_and_only_restart_private_core() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let (_t, mut c) = fixture();
    config(&c);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    c.readiness_url = format!("http://{}/health/ready", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let (mut s, _) = listener.accept().await.unwrap();
        let mut input = [0u8; 4096];
        let _ = s.read(&mut input).await;
        let body = "{\"status\":\"ok\"}";
        s.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).as_bytes()).await.unwrap();
    });
    let result = c.install().await.unwrap();
    assert_eq!(result["existing_timers_preserved"], true);
    let log = calls(c.tool_dir.as_ref().unwrap());
    assert!(log.contains("systemctl --user restart cbgpu-rust-core.service"));
    assert!(!log.contains(".timer"));
    let workerlog = calls(&c.root.join("target/release"));
    assert!(
        workerlog.contains("study-auto --dry-run")
            && workerlog.contains("timetable-refresh --dry-run")
    );
    let backup = PathBuf::from(result["rollback"].as_str().unwrap());
    let rollback = c.rollback(&backup).await.unwrap();
    assert_eq!(rollback["database_rolled_back"], false);
    assert!(!c.home.join(".local/lib/cbgpu-rust/current").exists());
    assert!(
        calls(c.tool_dir.as_ref().unwrap())
            .contains("systemctl --user stop cbgpu-rust-core.service")
    );
    server.await.unwrap();
}
#[tokio::test]
async fn install_restart_failure_restores_previous_binary_and_units() {
    let (_t, c) = fixture();
    config(&c);
    let base = c.home.join(".local/lib/cbgpu-rust");
    let old = base.join("releases/old");
    let units = c.home.join(".config/systemd/user");
    fs::create_dir_all(&old).unwrap();
    fs::create_dir_all(&units).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&old, base.join("current")).unwrap();
    for unit in cbgpu_ops::UNITS {
        fs::write(units.join(unit), "old-unit").unwrap();
    }
    fs::write(c.tool_dir.as_ref().unwrap().join("fail-restart-once"), "").unwrap();
    assert!(c.install().await.is_err());
    assert_eq!(
        base.join("current").canonicalize().unwrap(),
        old.canonicalize().unwrap()
    );
    for unit in cbgpu_ops::UNITS {
        assert_eq!(fs::read_to_string(units.join(unit)).unwrap(), "old-unit");
    }
    let log = calls(c.tool_dir.as_ref().unwrap());
    assert_eq!(log.matches("restart cbgpu-rust-core.service").count(), 2);
    assert!(!log.contains(".timer"));
}
