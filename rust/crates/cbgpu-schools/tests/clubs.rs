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
fn club_campaigns_are_school_scoped_clock_authorized_and_private() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let db = Database(format!("cbgpu-club-campaign-{}", std::process::id()));
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

    let effective = std::fs::read_to_string(
        root.join("supabase/migrations/20261117065015_effective_school_feature_policy.sql"),
    )
    .unwrap();
    let start = effective
        .find("create or replace function cbgpu_schools.feature_enabled")
        .unwrap();
    let end = effective[start..].find("revoke all on function").unwrap() + start;
    db.sql(&effective[start..end]);
    db.sql("INSERT INTO cbgpu_schools.schools(id,official_name,normalized_name,display_name,type,region,configuration,features,status) SELECT 'before-'||status,'Before '||status,'before '||status,'Before '||status,'MIDDLE_SCHOOL','충북','{}',ARRAY['community'],status FROM unnest(ARRAY['APPROVED','LISTED','UNCLAIMED','SUSPENDED'])status; INSERT INTO cbgpu_schools.schools(id,official_name,normalized_name,display_name,type,region,configuration,features,status) VALUES('before-existing-clubs','Before existing','before existing','Before existing','MIDDLE_SCHOOL','충북','{}',ARRAY['community','clubs'],'APPROVED'); CREATE TABLE clubs_before_school_rows AS SELECT id,to_jsonb(s) value FROM cbgpu_schools.schools s; CREATE TABLE clubs_before_memberships AS SELECT id,to_jsonb(m) value FROM cbgpu_schools.memberships m;");
    let migration = include_str!(
        "../../../../supabase/migrations/20261123145544_school_club_recruitment_campaigns.sql"
    );
    db.sql(migration);
    db.sql(migration);
    db.sql("DO $$BEGIN IF EXISTS(SELECT FROM clubs_before_school_rows b JOIN cbgpu_schools.schools s USING(id) WHERE (b.value-'features'-'revision') IS DISTINCT FROM(to_jsonb(s)-'features'-'revision')) OR EXISTS(SELECT id,value FROM clubs_before_memberships EXCEPT SELECT id,to_jsonb(m) FROM cbgpu_schools.memberships m) THEN RAISE EXCEPTION 'BACKFILL_CHANGED_UNRELATED_IDENTITY';END IF; IF EXISTS(SELECT FROM clubs_before_school_rows b JOIN cbgpu_schools.schools s USING(id) WHERE b.value->>'status'<>'APPROVED' AND b.value IS DISTINCT FROM to_jsonb(s)) OR NOT EXISTS(SELECT FROM cbgpu_schools.schools WHERE id='before-APPROVED' AND features=ARRAY['community','clubs'] AND revision=2) THEN RAISE EXCEPTION 'APPROVED_ONLY_BACKFILL';END IF;END$$; UPDATE cbgpu_schools.schools SET features=ARRAY['community'],revision=revision+1 WHERE id='before-APPROVED';");
    db.sql(migration);
    db.sql("DO $$BEGIN IF 'clubs'=ANY((SELECT features FROM cbgpu_schools.schools WHERE id='before-APPROVED')::text[]) THEN RAISE EXCEPTION 'RERUN_REMOVED_OPTOUT';END IF; END$$;");

    db.sql(include_str!("clubs.sql"));
    let spawn_review = |applicant: u32| {
        let query = format!(
            "BEGIN; SET LOCAL statement_timeout='5s'; SELECT set_config('request.jwt.claims','{{\"sub\":\"00000000-0000-4000-8000-000000000005\",\"session_id\":\"10000000-0000-4000-8000-000000000005\"}}',true); SELECT public.cbgpu_school_clubs_api('application_review',jsonb_build_object('school_id','clubs-school-1','club_id',club_id,'application_id',id,'expected_revision',revision,'status','APPROVED')) FROM cbgpu_schools.club_applications WHERE applicant_id=('00000000-0000-4000-8000-'||lpad('{applicant}',12,'0'))::uuid; SELECT pg_sleep(0.2); COMMIT;"
        );
        let mut child = Command::new("docker")
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
            .write_all(query.as_bytes())
            .unwrap();
        child
    };
    let first = spawn_review(6);
    let second = spawn_review(7);
    let a = first.wait_with_output().unwrap();
    let b = second.wait_with_output().unwrap();
    assert_ne!(
        a.status.success(),
        b.status.success(),
        "exactly one capacity-one approval must commit"
    );
    let denied = if a.status.success() { b } else { a };
    assert!(String::from_utf8_lossy(&denied.stderr).contains("CLUB_CAPACITY_FULL"));
    db.sql(include_str!("clubs_boundaries.sql"));
    db.sql("CREATE TABLE clubs_before_rollback_choices AS SELECT id,to_jsonb(s) value FROM cbgpu_schools.schools s WHERE id IN('before-APPROVED','before-existing-clubs','clubs-school-1','clubs-school-2');");
    db.sql(include_str!("../../../../operations/clubs/rollback.sql"));
    db.sql(include_str!("../../../../operations/clubs/rollback.sql"));
    db.sql("DO $$BEGIN IF EXISTS(SELECT FROM clubs_before_rollback_choices b JOIN cbgpu_schools.schools s USING(id) WHERE b.value IS DISTINCT FROM to_jsonb(s)) OR 'clubs'=ANY((SELECT features FROM cbgpu_schools.schools WHERE id='cbsh')::text[]) OR(SELECT count(*) FROM cbgpu_schools.club_applications)<>2 OR(SELECT count(*) FROM cbgpu_schools.club_campaigns)<>2 THEN RAISE EXCEPTION 'ROLLBACK_LOST_CHOICES_OR_HISTORY';END IF;END$$;");
}
