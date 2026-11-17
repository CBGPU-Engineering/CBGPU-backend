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
            "synthetic GPU enrollment SQL failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap()
    }
}
#[test]
#[ignore = "network-none disposable PostgreSQL; synthetic tokens only"]
fn gpu_inventory_enrollment_scoped_and_revocable() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let db = Database(format!(
        "cbgpu-gpu-enrollment-fixture-{}",
        std::process::id()
    ));
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
    db.sql("create role service_role nologin;create schema extensions;create extension pgcrypto with schema extensions;create function auth.jwt() returns jsonb language sql stable as $$select nullif(current_setting('request.jwt.claims',true),'')::jsonb$$;create function auth.role() returns text language sql stable as $$select auth.jwt()->>'role'$$;");
    for migration in [
        "20261117011230_multi_school_identity_onboarding.sql",
        "20261117020000_school_gpu_enrollment.sql",
    ] {
        let text =
            std::fs::read_to_string(root.join("supabase/migrations").join(migration)).unwrap();
        db.sql(&text);
        db.sql(&text);
    }
    db.sql(include_str!("postgres.sql"));
    let minted=db.sql("insert into auth.sessions(id,user_id) values('10000000-0000-4000-8000-000000000004','00000000-0000-4000-8000-000000000004');select set_config('request.jwt.claims','{\"sub\":\"00000000-0000-4000-8000-000000000004\",\"session_id\":\"10000000-0000-4000-8000-000000000004\",\"role\":\"authenticated\"}',false);select public.school_gpu_admin('cbsh','mint','{\"name\":\"Concurrent synthetic\"}');");
    let value: serde_json::Value = serde_json::from_str(
        minted
            .lines()
            .find(|line| line.contains("gpuenr_"))
            .unwrap(),
    )
    .unwrap();
    let token = value["token"].as_str().unwrap();
    let sql = format!(
        "begin;select public.school_gpu_enroll('{token}',repeat('a',64));select pg_sleep(0.2);commit;"
    );
    let mut children = vec![];
    for _ in 0..2 {
        children.push(
            Command::new("docker")
                .args([
                    "exec",
                    "-i",
                    &db.0,
                    "psql",
                    "-h",
                    "127.0.0.1",
                    "-U",
                    "postgres",
                    "-v",
                    "ON_ERROR_STOP=1",
                    "-At",
                    "-c",
                    &sql,
                ])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap(),
        );
    }
    let accepted = children
        .into_iter()
        .map(|mut child| child.wait().unwrap().success())
        .filter(|success| *success)
        .count();
    assert_eq!(
        accepted, 1,
        "exactly one concurrent enrollment may consume the capability"
    );
}
