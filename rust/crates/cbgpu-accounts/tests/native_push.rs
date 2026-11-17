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
fn sticky_native_capability_fresh_session_owner_rls_and_legacy_continuity() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let db = Database(format!("cbgpu-native-push-contract-{}", std::process::id()));
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
    db.sql(&std::fs::read_to_string(root.join("schema/tests/native-push.sql")).unwrap());
    let before =
        db.sql("select md5(jsonb_agg(to_jsonb(t)order by token)::text)from device_tokens t;");
    let migration = std::fs::read_to_string(
        root.join("supabase/migrations/20261117003335_native_push_owner_bound_capability.sql"),
    )
    .unwrap();
    db.sql(&migration);
    db.sql(&migration);
    assert_eq!(before,db.sql("select md5(jsonb_agg(to_jsonb(t)-'native_kind'order by token)::text)from device_tokens t;"));
    assert!(
        db.sql(
            &std::fs::read_to_string(root.join("schema/tests/native-push-enforcement.sql"))
                .unwrap()
        )
        .contains("NATIVE_PUSH_PRIVACY_TESTS_PASSED")
    );
}
