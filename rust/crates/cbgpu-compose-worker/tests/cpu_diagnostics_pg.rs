//! Explicit disposable PostgreSQL fixture. Never connects to a configured database.
use std::{
    io::Write,
    process::{Command, Stdio},
};
struct Fixture(String);
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = Command::new("docker").args(["rm", "-f", &self.0]).output();
    }
}
fn sql(name: &str, script: &str) -> String {
    let mut child = Command::new("docker")
        .args([
            "exec",
            "-i",
            name,
            "psql",
            "-U",
            "postgres",
            "-v",
            "ON_ERROR_STOP=1",
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
        .write_all(script.as_bytes())
        .unwrap();
    let result = child.wait_with_output().unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    String::from_utf8_lossy(&result.stdout).into_owned()
}
#[test]
#[ignore = "Creates an isolated disposable PostgreSQL 17 Docker fixture; no host ports or production DB access"]
fn diagnostics_are_service_managed_even_with_table_wide_authenticated_update() {
    let name = format!("cbgpu-native-cpu-schema-{}", std::process::id());
    let fixture = Fixture(name.clone());
    let output = Command::new("docker")
        .args([
            "run",
            "--rm",
            "-d",
            "--name",
            &name,
            "--network",
            "none",
            "--memory",
            "256m",
            "--cpus",
            "1",
            "--tmpfs",
            "/var/lib/postgresql/data:rw,size=192m",
            "--label",
            "cbgpu.test=native-cpu-diagnostics",
            "-e",
            "POSTGRES_PASSWORD=fixture-only",
            "postgres:17-alpine",
        ])
        .output()
        .unwrap();
    assert!(output.status.success());
    let mut ready = false;
    for _ in 0..100 {
        // pg_isready can see the temporary initialization server just before it
        // exits. Wait for the entrypoint's final-server marker as well.
        let initialized = Command::new("docker")
            .args(["logs", &name])
            .output()
            .is_ok_and(|r| {
                String::from_utf8_lossy(&r.stdout).contains("PostgreSQL init process complete")
            });
        if initialized
            && Command::new("docker")
                .args(["exec", &name, "pg_isready", "-U", "postgres"])
                .output()
                .is_ok_and(|r| r.status.success())
        {
            ready = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    assert!(ready);
    sql(
        &name,
        "CREATE ROLE authenticated; CREATE ROLE anon; CREATE ROLE service_role; CREATE TABLE public.cpu_projects(id integer primary key, name text); GRANT USAGE ON SCHEMA public TO authenticated,anon,service_role; GRANT ALL ON public.cpu_projects TO authenticated,anon,service_role;",
    );
    let migration = include_str!(
        "../../../../supabase/migrations/20261116120100_cpu_deployment_diagnostics.sql"
    );
    sql(&name, migration);
    sql(&name, migration);
    sql(
        &name,
        r#"
 SET ROLE authenticated;
 INSERT INTO public.cpu_projects(id,name)VALUES(1,'owned project');
 UPDATE public.cpu_projects SET name='updated safely' WHERE id=1;
 DO $$ BEGIN
  BEGIN UPDATE public.cpu_projects SET deployment_error='forged success' WHERE id=1; RAISE EXCEPTION 'forged diagnostics accepted'; EXCEPTION WHEN insufficient_privilege THEN NULL; END;
  BEGIN INSERT INTO public.cpu_projects(id,deployment_logs)VALUES(2,'forged'); RAISE EXCEPTION 'forged insert accepted'; EXCEPTION WHEN insufficient_privilege THEN NULL; END;
 END $$;
 RESET ROLE; SET ROLE service_role;
 UPDATE public.cpu_projects SET deployment_error='redacted failure',deployment_logs='service output',deployment_attempted_at=now(),deployment_diagnostics='{"rollback_restored":true}' WHERE id=1;
 RESET ROLE; SET ROLE authenticated;
 UPDATE public.cpu_projects SET name='still editable' WHERE id=1;
 DO $$ BEGIN
  IF (SELECT deployment_error FROM public.cpu_projects WHERE id=1) <> 'redacted failure' THEN RAISE EXCEPTION 'service diagnostics lost'; END IF;
  BEGIN UPDATE public.cpu_projects SET deployment_logs=NULL WHERE id=1; RAISE EXCEPTION 'clearing diagnostics accepted'; EXCEPTION WHEN insufficient_privilege THEN NULL; END;
 END $$;
 RESET ROLE;SET ROLE anon;
 DO $$ BEGIN BEGIN UPDATE public.cpu_projects SET deployment_diagnostics='{}' WHERE id=1; RAISE EXCEPTION 'anonymous diagnostics accepted'; EXCEPTION WHEN insufficient_privilege THEN NULL; END; END $$;
 RESET ROLE;
 DO $$ BEGIN BEGIN UPDATE public.cpu_projects SET deployment_error=repeat('x',2001) WHERE id=1;RAISE EXCEPTION 'unbounded diagnostics accepted';EXCEPTION WHEN check_violation THEN NULL;END;END $$;
 "#,
    );
    drop(fixture);
}
