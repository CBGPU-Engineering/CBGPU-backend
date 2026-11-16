//! A network-isolated PostgreSQL fixture, reached only through its temporary Unix socket.
use cbgpu_contest_teams::allocate;
use sqlx::{
    Row,
    postgres::{PgConnectOptions, PgPoolOptions},
};
use std::{
    io::Write,
    os::unix::fs::PermissionsExt,
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
    fn sql(&self, text: &str) -> String {
        let mut child = Command::new("docker")
            .args([
                "exec",
                "-i",
                &self.0,
                "psql",
                "-h",
                "/socket",
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
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap()
    }
}
#[tokio::test]
#[ignore = "requires Docker; only creates disposable network-none PostgreSQL"]
async fn least_privilege_deadline_concurrency_and_no_reassignment() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let socket = tempfile::tempdir().unwrap();
    std::fs::set_permissions(socket.path(), std::fs::Permissions::from_mode(0o777)).unwrap();
    let db = Database(format!("cbgpu-team-contract-{}", std::process::id()));
    let out = Command::new("docker")
        .args([
            "run",
            "-d",
            "--name",
            &db.0,
            "--network",
            "none",
            "--memory",
            "384m",
            "--cpus",
            "1",
            "--tmpfs",
            "/var/lib/postgresql/data:rw,size=256m",
            "-v",
            &format!("{}:/socket", socket.path().display()),
            "-e",
            "POSTGRES_HOST_AUTH_METHOD=trust",
            "-e",
            "PGHOST=/socket",
            "postgres:17-alpine",
            "postgres",
            "-k",
            "/var/run/postgresql,/socket",
        ])
        .output()
        .unwrap();
    assert!(out.status.success(), "fixture start failed");
    let mut ready = false;
    for _ in 0..150 {
        let logs = Command::new("docker")
            .args(["logs", &db.0])
            .output()
            .unwrap();
        if String::from_utf8_lossy(&logs.stdout).contains("PostgreSQL init process complete")
            && Command::new("docker")
                .args([
                    "exec",
                    &db.0,
                    "pg_isready",
                    "-h",
                    "/socket",
                    "-U",
                    "postgres",
                ])
                .output()
                .is_ok_and(|o| o.status.success())
        {
            ready = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    if !ready {
        let logs = Command::new("docker")
            .args(["logs", &db.0])
            .output()
            .unwrap();
        panic!(
            "fixture initialization: {} {}",
            String::from_utf8_lossy(&logs.stdout),
            String::from_utf8_lossy(&logs.stderr)
        );
    }
    for file in [
        "schema/tests/contests/bootstrap.sql",
        "schema/legacy/web-supabase/migrations/20261018051657_contest_submissions.sql",
        "schema/legacy/web-supabase/migrations/20261018052904_contest_logos.sql",
        "schema/legacy/web-supabase/migrations/20261018114551_event_management.sql",
        "schema/legacy/web-supabase/migrations/20261018120743_event_operations.sql",
        "supabase/migrations/20261116080117_contest_presentation_assets.sql",
        "supabase/migrations/20261116083445_contest_team_registration.sql",
    ] {
        db.sql(&std::fs::read_to_string(root.join(file)).unwrap());
    }
    db.sql("ALTER ROLE cbgpu_contest_teams LOGIN; INSERT INTO auth.users SELECT ('00000000-0000-4000-8000-'||lpad(i::text,12,'0'))::uuid FROM generate_series(1,20)i;");
    let contest = "10000000-0000-4000-8000-000000000001";
    db.sql(&format!("SET ROLE authenticated; SELECT set_config('request.jwt.claim.sub','00000000-0000-4000-8000-000000000001',false); INSERT INTO public.contests(id,host_id,title,fields,participation_mode,team_min_size,team_max_size,allow_individual_entries,event_start_at,registration_closes_at) VALUES('{contest}','00000000-0000-4000-8000-000000000001','Synthetic teams','[{{\"id\":\"20000000-0000-4000-8000-000000000001\",\"type\":\"text\",\"label\":\"Result\",\"description\":\"\",\"required\":false}}]','team',3,4,true,clock_timestamp()+interval '2 days',clock_timestamp()+interval '1 day');"));
    for i in 2..=8 {
        db.sql(&format!("SET ROLE authenticated;SELECT set_config('request.jwt.claim.sub','00000000-0000-4000-8000-{i:012}',false);SELECT * FROM public.register_contest_entry('{contest}','Synthetic {i}','{{}}','individual');"));
    }
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect_with(
            PgConnectOptions::new()
                .host(socket.path().to_str().unwrap())
                .username("cbgpu_contest_teams")
                .database("postgres"),
        )
        .await
        .unwrap();
    assert_eq!(
        allocate(&pool, false).await.unwrap().assigned,
        0,
        "before deadline"
    );
    db.sql(&format!("UPDATE public.contests SET registration_closes_at=clock_timestamp()-interval '1 minute' WHERE id='{contest}';"));
    assert_eq!(
        allocate(&pool, true).await.unwrap().assigned,
        7,
        "dry run previews allocation"
    );
    assert_eq!(
        db.sql("SELECT count(*) FROM public.contest_teams").trim(),
        "0",
        "dry run rolls back"
    );
    let (a, b) = tokio::join!(allocate(&pool, false), allocate(&pool, false));
    let a = a.unwrap();
    let b = b.unwrap();
    assert_eq!(a.assigned + b.assigned, 7);
    assert_eq!(a.created_teams + b.created_teams, 2);
    let membership =
        db.sql("SELECT user_id,team_id FROM public.contest_team_members ORDER BY user_id;");
    assert_eq!(allocate(&pool, false).await.unwrap().assigned, 0);
    assert_eq!(
        membership,
        db.sql("SELECT user_id,team_id FROM public.contest_team_members ORDER BY user_id;")
    );
    let counts = sqlx::query(
        "SELECT count(*) AS n FROM public.contest_team_members GROUP BY team_id ORDER BY n",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        counts
            .iter()
            .map(|r| r.get::<i64, _>("n"))
            .collect::<Vec<_>>(),
        vec![3, 4]
    );
    // The scheduler has no write permission on users, submissions, registration answers or event settings.
    assert!(
        sqlx::query("UPDATE public.contests SET team_min_size=1")
            .execute(&pool)
            .await
            .is_err()
    );
    assert!(
        sqlx::query("SELECT answers FROM public.event_registrations")
            .fetch_all(&pool)
            .await
            .is_err()
    );
    assert!(
        sqlx::query("SELECT * FROM auth.users")
            .fetch_all(&pool)
            .await
            .is_err()
    );
    pool.close().await;
}
