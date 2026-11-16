//! Real independent connections, never a production URL: PostgreSQL has network=none.
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use std::{
    io::Write,
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    process::{Command, Stdio},
    time::Duration,
};
use uuid::Uuid;
struct Fixture(String);
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = Command::new("docker")
            .args(["rm", "-f", "-v", &self.0])
            .output();
    }
}
impl Fixture {
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
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    }
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires Docker; disposable network-none database only, no production credentials"]
async fn awards_privacy_and_concurrent_ballot_upsert() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let socket = tempfile::tempdir().unwrap();
    std::fs::set_permissions(socket.path(), std::fs::Permissions::from_mode(0o777)).unwrap();
    let db = Fixture(format!(
        "cbgpu-awards-contract-{}-{}",
        std::process::id(),
        Uuid::new_v4().simple()
    ));
    let output = Command::new("docker")
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
    assert!(
        output.status.success(),
        "Could not start disposable awards fixture"
    );
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
                .is_ok_and(|r| r.status.success())
        {
            ready = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(ready, "Disposable awards database did not become ready");
    for file in [
        "schema/tests/contests/bootstrap.sql",
        "schema/legacy/web-supabase/migrations/20261018051657_contest_submissions.sql",
        "schema/legacy/web-supabase/migrations/20261018052904_contest_logos.sql",
        "schema/legacy/web-supabase/migrations/20261018114551_event_management.sql",
        "schema/legacy/web-supabase/migrations/20261018120743_event_operations.sql",
        "supabase/migrations/20261116080117_contest_presentation_assets.sql",
        "supabase/migrations/20261116083445_contest_team_registration.sql",
        "supabase/migrations/20261116103228_contest_start_windows.sql",
        "supabase/migrations/20261116105025_contest_awards_voting.sql",
        "schema/tests/contests/awards.sql",
    ] {
        db.sql(&std::fs::read_to_string(root.join(file)).unwrap());
    }
    db.sql("INSERT INTO auth.users SELECT ('00000000-0000-4000-8000-'||lpad(i::text,12,'0'))::uuid FROM generate_series(1,4)i;");
    let contest = Uuid::parse_str("90000000-0000-4000-8000-000000000001").unwrap();
    let host = "00000000-0000-4000-8000-000000000001";
    db.sql(&format!("SET ROLE authenticated;SELECT set_config('request.jwt.claim.sub','{host}',false);INSERT INTO contests(id,host_id,title,fields)VALUES('{contest}','{host}','Concurrent awards','[{{\"id\":\"20000000-0000-4000-8000-000000000001\",\"type\":\"text\",\"label\":\"Private\",\"description\":\"\",\"required\":false}}]');SELECT contest_awards_api('{contest}','configure',jsonb_build_object('enabled',true,'closes_at',clock_timestamp()+interval '1 hour'));"));
    for user in 2..=4 {
        let actor = format!("00000000-0000-4000-8000-{user:012}");
        db.sql(&format!("SET ROLE authenticated;SELECT set_config('request.jwt.claim.sub','{actor}',false);SELECT register_contest_entry('{contest}','Synthetic participant','{{}}');"));
        if user < 4 {
            db.sql(&format!("SET ROLE authenticated;SELECT set_config('request.jwt.claim.sub','{actor}',false);INSERT INTO contest_submissions(contest_id,user_id,participant_name,answers)VALUES('{contest}','{actor}','Private participant','{{}}');SELECT contest_awards_api('{contest}','showcase',jsonb_build_object('submission_user_id','{actor}','title','Public card {user}'));"));
        }
    }
    let category:Uuid=db.sql(&format!("SELECT id FROM contest_award_categories WHERE contest_id='{contest}' ORDER BY position LIMIT 1;")).trim().parse().unwrap();
    let choices: Vec<Uuid> = db
        .sql(&format!(
            "SELECT id FROM contest_showcases WHERE contest_id='{contest}' ORDER BY title;"
        ))
        .lines()
        .map(|line| line.parse().unwrap())
        .collect();
    assert_eq!(choices.len(), 2);
    let pool = PgPoolOptions::new()
        .max_connections(4)
        .connect_with(
            PgConnectOptions::new()
                .host(socket.path().to_str().unwrap())
                .username("postgres")
                .database("postgres"),
        )
        .await
        .unwrap();
    async fn vote(pool: &sqlx::PgPool, contest: Uuid, category: Uuid, showcase: Uuid) -> String {
        let mut tx = pool.begin().await.unwrap();
        sqlx::query("SET LOCAL ROLE authenticated")
            .execute(&mut *tx)
            .await
            .unwrap();
        sqlx::query("SELECT set_config('request.jwt.claim.sub','00000000-0000-4000-8000-000000000004',true)").execute(&mut *tx).await.unwrap();
        let result:String=sqlx::query_scalar("SELECT contest_awards_api($1,'vote',jsonb_build_object('category_id',$2::uuid,'showcase_id',$3::uuid))::text").bind(contest).bind(category).bind(showcase).fetch_one(&mut *tx).await.unwrap();
        tx.commit().await.unwrap();
        result
    }
    let (first, second) = tokio::join!(
        vote(&pool, contest, category, choices[0]),
        vote(&pool, contest, category, choices[1])
    );
    for result in [first, second] {
        let json: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(json["ok"], true);
    }
    let count:i64=sqlx::query_scalar("SELECT count(*) FROM contest_award_ballots WHERE contest_id=$1 AND category_id=$2 AND voter_id='00000000-0000-4000-8000-000000000004'").bind(contest).bind(category).fetch_one(&pool).await.unwrap();
    assert_eq!(
        count, 1,
        "Concurrent changes must retain exactly one ballot"
    );
    let selected:Uuid=sqlx::query_scalar("SELECT showcase_id FROM contest_award_ballots WHERE category_id=$1 AND voter_id='00000000-0000-4000-8000-000000000004'").bind(category).fetch_one(&pool).await.unwrap();
    assert!(choices.contains(&selected));
    pool.close().await;
}
