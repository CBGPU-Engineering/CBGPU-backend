use serde_json::{Value, json};
use std::process::Command;
fn binary() -> Command {
    Command::new(env!("CARGO_BIN_EXE_cbgpu-domains"))
}
#[test]
fn custom_tls_missing_credentials_retains_legacy_json_exit_zero() {
    let root = tempfile::tempdir().unwrap();
    let out = binary()
        .args([
            "--root",
            root.path().to_str().unwrap(),
            "custom-tls",
            "ensure",
            "research.example.org",
        ])
        .output()
        .unwrap();
    assert!(out.status.success());
    assert!(out.stderr.is_empty());
    assert_eq!(
        serde_json::from_slice::<Value>(&out.stdout).unwrap(),
        json!({"status":"provider_setup_required","records":[]})
    );
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
}
#[test]
fn project_adapter_failure_is_nonzero_without_secret_path_disclosure() {
    let root = tempfile::tempdir().unwrap();
    let out = binary()
        .args([
            "--root",
            root.path().to_str().unwrap(),
            "cloudflare",
            "ensure",
            "demo.front.cbgpu.com",
        ])
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(out.stdout.is_empty());
    let error = String::from_utf8(out.stderr).unwrap();
    assert!(error.contains("credentials are unavailable"));
    assert!(!error.contains(root.path().to_str().unwrap()));
}
#[test]
fn custom_tls_remove_missing_credentials_is_also_json_compatible() {
    let root = tempfile::tempdir().unwrap();
    let out = binary()
        .args([
            "--root",
            root.path().to_str().unwrap(),
            "custom-tls",
            "remove",
            "provider-123",
        ])
        .output()
        .unwrap();
    assert!(out.status.success());
    assert_eq!(
        serde_json::from_slice::<Value>(&out.stdout).unwrap(),
        json!({"status":"provider_setup_required","records":[]})
    );
}
#[test]
fn help_and_version_do_not_load_credentials_or_contact_provider() {
    let root = tempfile::tempdir().unwrap();
    for arg in ["--help", "--version"] {
        let out = binary().arg(arg).current_dir(root.path()).output().unwrap();
        assert!(out.status.success());
        assert!(!out.stdout.is_empty());
        assert!(out.stderr.is_empty());
    }
}
