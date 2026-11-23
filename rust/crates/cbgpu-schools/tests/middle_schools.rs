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
        format!(
            "{}{}",
            String::from_utf8(out.stdout).unwrap(),
            String::from_utf8_lossy(&out.stderr)
        )
    }
}
#[test]
#[ignore = "network-none disposable PostgreSQL; synthetic tokens only"]
fn middle_school_catalog_is_listed_only_and_participation_is_explicit() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let db = Database(format!("cbgpu-middle-catalog-{}", std::process::id()));
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
    db.sql(
        &std::fs::read_to_string(
            root.join("supabase/migrations/20261117125257_contest_participant_school_policy.sql"),
        )
        .unwrap(),
    );
    db.sql(
        &std::fs::read_to_string(
            root.join("supabase/migrations/20261117013206_multi_school_features_messaging.sql"),
        )
        .unwrap(),
    );
    db.sql(
        &std::fs::read_to_string(
            root.join("supabase/migrations/20261117030138_school_life_source_registry.sql"),
        )
        .unwrap(),
    );
    db.sql("CREATE TABLE original_life_definition AS SELECT pg_get_functiondef('public.cbgpu_school_life_sources(text)'::regprocedure) definition;");
    db.sql(include_str!(
        "../../../../supabase/migrations/20261123141606_canonical_public_school_directory.sql"
    ));
    db.sql(include_str!(
        "../../../../supabase/migrations/20261123143055_optimize_canonical_school_candidates.sql"
    ));
    db.sql("CREATE TABLE original_school_rows AS SELECT id,to_jsonb(s) value FROM cbgpu_schools.schools s; CREATE TABLE original_membership_rows AS SELECT id,to_jsonb(m) value FROM cbgpu_schools.memberships m;");
    let migration = include_str!(
        "../../../../supabase/migrations/20261123144307_middle_school_catalog_and_participant_policies.sql"
    );
    db.sql(migration);
    db.sql(migration);
    let manifest = include_str!("../../../../schema/data/neis-middle-schools-2026-10-31.json");
    let manifest = manifest.replace("'", "''");
    let import = include_str!("../../../../operations/middle-schools/import.sql.template")
        .replace("__MIDDLE_MANIFEST_SQL_LITERAL__", &manifest);
    db.sql(&import);
    db.sql(&import);
    db.sql(include_str!("middle_schools.sql"));
    db.sql("DELETE FROM public.contests WHERE participant_policy NOT IN('ALL_ELIGIBLE','HIGH_SCHOOL_STUDENTS');");
    db.sql(include_str!(
        "../../../../operations/middle-schools/rollback.sql"
    ));
    db.sql(include_str!(
        "../../../../operations/middle-schools/rollback.sql"
    ));
    db.sql(migration);
    db.sql("DO $$BEGIN IF (SELECT count(*) FROM cbgpu_schools.schools WHERE type='MIDDLE_SCHOOL' AND source_sha256 IS NOT NULL)<>3358 THEN RAISE EXCEPTION 'ROLLBACK_CATALOG_LOSS';END IF;END$$;");
}
