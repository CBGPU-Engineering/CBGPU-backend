//! Run with `cargo test -p cbgpu-accounts --test postgres -- --ignored`.
//! Creates only a bounded, network-isolated, disposable PostgreSQL container.
use std::{
    io::Write,
    path::PathBuf,
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
    fn sql(&self, sql: &str) -> String {
        let mut child = Command::new("docker")
            .args([
                "exec",
                "-i",
                &self.0,
                "psql",
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
            .write_all(sql.as_bytes())
            .unwrap();
        let out = child.wait_with_output().unwrap();
        assert!(
            out.status.success(),
            "isolated SQL failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap()
    }
}
#[test]
#[ignore = "requires Docker; synthetic network-none PostgreSQL only"]
fn registration_guest_rollover_project_continuity_and_replay() {
    let root = std::env::var_os("CBGPU_ACCOUNT_REPO")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.."));
    let db = Database(format!("cbgpu-account-contract-{}", std::process::id()));
    let out = Command::new("docker")
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
            "postgres:17-alpine",
        ])
        .output()
        .unwrap();
    assert!(out.status.success(), "could not start isolated database");
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
    assert!(ready, "isolated DB did not become ready");
    db.sql(&std::fs::read_to_string(root.join("schema/tests/accounts/bootstrap.sql")).unwrap());
    let migration = std::fs::read_to_string(
        root.join("supabase/migrations/20261116084450_account_access_rollover.sql"),
    )
    .unwrap();
    db.sql(&migration);
    let results =
        db.sql(&std::fs::read_to_string(root.join("schema/tests/accounts/workflows.sql")).unwrap());
    assert!(results.contains("ACCOUNT_WORKFLOW_TESTS_PASSED"));
    assert!(results.contains("ACCOUNT_TRANSACTION_ROLLBACK_PASSED"));
    db.sql(
        &std::fs::read_to_string(root.join("schema/tests/accounts/contest-boundaries.sql"))
            .unwrap(),
    );
    let contest = std::fs::read_to_string(
        root.join("supabase/migrations/20261116233927_account_contest_execution_guard.sql"),
    )
    .unwrap();
    db.sql(&contest);
    db.sql(&contest);
    assert!(
        db.sql(
            &std::fs::read_to_string(root.join("schema/tests/accounts/contest-enforcement.sql"))
                .unwrap()
        )
        .contains("ACCOUNT_CONTEST_BOUNDARIES_PASSED")
    );
    db.sql("RESET ROLE;");
    let snapshot = "select md5(jsonb_agg(to_jsonb(p) order by id)::text) from public.profiles p;";
    let before = db.sql(snapshot);
    db.sql(&migration);
    assert_eq!(
        before,
        db.sql(snapshot),
        "migration replay changed accounts"
    );
    let signup_review = std::fs::read_to_string(
        root.join("supabase/migrations/20261124122757_signup_review_after_activation.sql"),
    )
    .unwrap();
    db.sql(&signup_review);
    db.sql(&signup_review);
    assert_eq!(
        before,
        db.sql(snapshot),
        "signup migration touched existing accounts"
    );
    assert!(
        db.sql(
            &std::fs::read_to_string(root.join("schema/tests/accounts/signup-review.sql")).unwrap()
        )
        .contains("SIGNUP_REVIEW_PASSED")
    );
}
