//! Explicit Docker-only synthetic privacy fixture; no push sends, host ports or real tokens.
use std::{
    io::Write,
    process::{Command, Stdio},
    time::Duration,
};
struct Database(String);
impl Drop for Database {
    fn drop(&mut self) {
        let _ = Command::new("docker")
            .args(["rm", "-f", "-v", &self.0])
            .output();
    }
}
impl Database {
    fn sql(&self, text: &str) -> String {
        let mut child = Command::new("docker")
            .args([
                "exec",
                "-i",
                &self.0,
                "psql",
                "-h",
                "127.0.0.1",
                "-U",
                "postgres",
                "-v",
                "ON_ERROR_STOP=1",
                "-At",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(text.as_bytes())
            .unwrap();
        let out = child.wait_with_output().unwrap();
        assert!(
            out.status.success(),
            "synthetic native push SQL failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap()
    }
}
#[test]
#[ignore = "network-none disposable PostgreSQL; synthetic tokens only"]
fn tenant_onboarding_resources_and_message_scopes() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let db = Database(format!("cbgpu-schools-contract-{}", std::process::id()));
    assert!(
        Command::new("docker")
            .args([
                "run",
                "--name",
                &db.0,
                "--network",
                "none",
                "--memory",
                "256m",
                "--cpus",
                "1",
                "-e",
                "POSTGRES_HOST_AUTH_METHOD=trust",
                "-d",
                "postgres:17-alpine"
            ])
            .output()
            .unwrap()
            .status
            .success()
    );
    let mut ready = false;
    for _ in 0..100 {
        if Command::new("docker")
            .args([
                "exec",
                &db.0,
                "pg_isready",
                "-h",
                "127.0.0.1",
                "-U",
                "postgres",
            ])
            .output()
            .unwrap()
            .status
            .success()
        {
            ready = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(ready);
    db.sql(&std::fs::read_to_string(root.join("schema/tests/schools/bootstrap.sql")).unwrap());
    for migration in [
        "20261117011230_multi_school_identity_onboarding.sql",
        "20261117012655_multi_school_resource_boundaries.sql",
        "20261117013206_multi_school_features_messaging.sql",
        "20261117015504_explicit_public_school_content.sql",
    ] {
        let text =
            std::fs::read_to_string(root.join("supabase/migrations").join(migration)).unwrap();
        db.sql(&text);
        db.sql(&text);
    }
    db.sql(&std::fs::read_to_string(root.join("schema/tests/schools/contract.sql")).unwrap());
    db.sql(&std::fs::read_to_string(root.join("schema/tests/schools/resources.sql")).unwrap());
    db.sql(&std::fs::read_to_string(root.join("schema/tests/schools/public.sql")).unwrap());
}
