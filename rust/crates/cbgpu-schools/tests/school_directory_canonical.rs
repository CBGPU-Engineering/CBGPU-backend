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
            "canonical directory SQL failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap()
    }
}
#[test]
#[ignore = "network-none disposable PostgreSQL; synthetic tokens only"]
fn canonical_public_directory_preserves_school_identity() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let db = Database(format!("cbgpu-school-canonical-{}", std::process::id()));
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
    let base = std::fs::read_to_string(
        root.join("supabase/migrations/20261117011230_multi_school_identity_onboarding.sql"),
    )
    .unwrap();
    db.sql(&base);
    db.sql("CREATE ROLE service_role NOLOGIN;");
    db.sql(
        &std::fs::read_to_string(
            root.join("supabase/migrations/20261117012655_multi_school_resource_boundaries.sql"),
        )
        .unwrap(),
    );
    db.sql(
        &std::fs::read_to_string(
            root.join("supabase/migrations/20261117124920_school_catalog_bound_applications.sql"),
        )
        .unwrap(),
    );
    db.sql(include_str!("school_directory_canonical.sql"));
    let migration = include_str!(
        "../../../../supabase/migrations/20261123141606_canonical_public_school_directory.sql"
    );
    db.sql(migration);
    db.sql(migration);
    db.sql(include_str!("school_directory_canonical_assert.sql"));
    db.sql(include_str!(
        "../../../../operations/school-search/rollback.sql"
    ));
    db.sql(include_str!(
        "../../../../operations/school-search/rollback.sql"
    ));
    db.sql("DO $$BEGIN IF public.cbgpu_school_api('directory','{\"q\":\"설화고등학교\"}')->>'total'<>'2' THEN RAISE EXCEPTION 'ROLLBACK_FAILED';END IF;END$$;");
    db.sql(migration);
    db.sql("DO $$BEGIN IF public.cbgpu_school_api('directory','{\"q\":\"설화고등학교\"}')->>'total'<>'1' THEN RAISE EXCEPTION 'REAPPLY_FAILED';END IF;END$$;");
}
