//! Exact private oracle and existing contest SQL contracts on a disposable PG17 fixture.
use cbgpu_contests_api::auth::{PgSessions, Sessions};
const OWNER: &str = "00000000-0000-4000-8000-000000000001";
const OTHER: &str = "00000000-0000-4000-8000-000000000002";
const SESSION: &str = "10000000-0000-4000-8000-000000000001";
#[tokio::test]
#[ignore = "Requires disposable CBGPU_CONTEST_TEST_DATABASE_URL ending /cbgpu_contests_api_test and CBGPU_CONTEST_REPO"]
async fn real_pg17_oracle_observes_rls_revocation_and_existing_registration_rules() {
    let url = std::env::var("CBGPU_CONTEST_TEST_DATABASE_URL").unwrap();
    assert!(url.ends_with("/cbgpu_contests_api_test"));
    let repo = std::path::PathBuf::from(
        std::env::var("CBGPU_CONTEST_REPO").expect("Explicit reviewed backend checkout path"),
    );
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
        .unwrap();
    sqlx::raw_sql("CREATE ROLE anon NOLOGIN;CREATE ROLE authenticated NOLOGIN;CREATE SCHEMA auth;CREATE TABLE auth.users(id uuid PRIMARY KEY,deleted_at timestamptz,banned_until timestamptz,encrypted_password text);CREATE TABLE auth.sessions(id uuid PRIMARY KEY,user_id uuid,not_after timestamptz,secret text);CREATE TABLE public.profiles(id uuid PRIMARY KEY,is_approved boolean,account_status text);ALTER TABLE auth.users ENABLE ROW LEVEL SECURITY;ALTER TABLE auth.sessions ENABLE ROW LEVEL SECURITY;ALTER TABLE public.profiles ENABLE ROW LEVEL SECURITY;CREATE FUNCTION auth.jwt()RETURNS jsonb LANGUAGE sql STABLE AS $$SELECT coalesce(nullif(current_setting('request.jwt.claims',true),'')::jsonb,'{}'::jsonb)$$;CREATE FUNCTION auth.uid()RETURNS uuid LANGUAGE sql STABLE AS $$SELECT coalesce(nullif(current_setting('request.jwt.claim.sub',true),''),(auth.jwt()->>'sub'))::uuid$$;GRANT USAGE ON SCHEMA auth TO authenticated;GRANT EXECUTE ON FUNCTION auth.uid()TO authenticated;").execute(&pool).await.unwrap();
    let migration = std::fs::read_to_string(
        repo.join("supabase/migrations/20261116130154_contest_external_api_session_guard.sql"),
    )
    .unwrap();
    for _ in 0..2 {
        sqlx::raw_sql(sqlx::AssertSqlSafe(migration.as_str()))
            .execute(&pool)
            .await
            .unwrap();
    }
    let resource_oracle = std::fs::read_to_string(
        repo.join("supabase/migrations/20261116235440_contest_account_resource_oracle.sql"),
    )
    .unwrap();
    for _ in 0..2 {
        sqlx::raw_sql(sqlx::AssertSqlSafe(resource_oracle.as_str()))
            .execute(&pool)
            .await
            .unwrap();
    }
    for role in ["anon", "authenticated"] {
        let mut tx = pool.begin().await.unwrap();
        sqlx::query(sqlx::AssertSqlSafe(format!("SET LOCAL ROLE {role}")))
            .execute(&mut *tx)
            .await
            .unwrap();
        assert!(
            sqlx::query("SELECT * FROM cbgpu_contests_auth.session_status(NULL,NULL)")
                .fetch_all(&mut *tx)
                .await
                .is_err()
        );
        tx.rollback().await.unwrap();
    }
    let elevated:bool=sqlx::query_scalar("SELECT rolsuper OR rolbypassrls OR rolcanlogin OR rolcreaterole OR rolcreatedb FROM pg_roles WHERE rolname='cbgpu_contests_auth_reader'").fetch_one(&pool).await.unwrap();
    assert!(!elevated);
    let owner: uuid::Uuid = OWNER.parse().unwrap();
    let other: uuid::Uuid = OTHER.parse().unwrap();
    let session: uuid::Uuid = SESSION.parse().unwrap();
    sqlx::query("INSERT INTO auth.users(id,encrypted_password)VALUES($1,'must-not-read'),($2,'must-not-read')").bind(owner).bind(other).execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO public.profiles(id,is_approved)VALUES($1,true),($2,true)")
        .bind(owner)
        .bind(other)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO auth.sessions VALUES($1,$2,clock_timestamp()+interval'1 hour','must-not-read')").bind(session).bind(owner).execute(&pool).await.unwrap();
    let reader = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .after_connect(|connection, _| {
            Box::pin(async move {
                sqlx::query("SET ROLE cbgpu_contests_auth_reader")
                    .execute(&mut *connection)
                    .await?;
                sqlx::query("SET default_transaction_read_only=on")
                    .execute(connection)
                    .await?;
                Ok(())
            })
        })
        .connect(&url)
        .await
        .unwrap();
    let verifier = PgSessions { pool: reader };
    assert_eq!(verifier.active(session, owner).await.unwrap(), Some(true));
    assert_eq!(verifier.active(session, other).await.unwrap(), None);
    for sql in [
        "SELECT id FROM auth.users",
        "SELECT id FROM auth.sessions",
        "SELECT id FROM public.profiles",
        "SELECT encrypted_password FROM auth.users",
        "SELECT secret FROM auth.sessions",
        "UPDATE public.profiles SET is_approved=true",
        "CREATE TABLE forbidden(id int)",
    ] {
        assert!(
            sqlx::query(sql).execute(&verifier.pool).await.is_err(),
            "reader must reject {sql}"
        );
    }
    // Transaction-local claims must not survive returning the connection to the pool.
    let unmatched: (bool, bool) =
        sqlx::query_as("SELECT valid,approved FROM cbgpu_contests_auth.session_status($1,$2)")
            .bind(owner)
            .bind(session)
            .fetch_one(&verifier.pool)
            .await
            .unwrap();
    assert_eq!(unmatched, (false, false));
    sqlx::query("UPDATE public.profiles SET is_approved=false WHERE id=$1")
        .bind(owner)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(verifier.active(session, owner).await.unwrap(), Some(false));
    sqlx::query("UPDATE public.profiles SET account_status='SUSPENDED'WHERE id=$1")
        .bind(owner)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(verifier.active(session, owner).await.unwrap(), None);
    sqlx::query("UPDATE public.profiles SET account_status=NULL,is_approved=NULL WHERE id=$1")
        .bind(owner)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(verifier.active(session, owner).await.unwrap(), Some(true));
    sqlx::query(
        "UPDATE auth.sessions SET not_after=clock_timestamp()-interval'1 second'WHERE id=$1",
    )
    .bind(session)
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(verifier.active(session, owner).await.unwrap(), None);
    sqlx::query("UPDATE auth.sessions SET not_after=NULL WHERE id=$1")
        .bind(session)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(verifier.active(session, owner).await.unwrap(), Some(true));
    sqlx::query("UPDATE auth.users SET banned_until=clock_timestamp()+interval'1 hour'WHERE id=$1")
        .bind(owner)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(verifier.active(session, owner).await.unwrap(), None);
    sqlx::query("UPDATE auth.users SET banned_until=NULL,deleted_at=clock_timestamp()WHERE id=$1")
        .bind(owner)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(verifier.active(session, owner).await.unwrap(), None);
    sqlx::query("UPDATE auth.users SET deleted_at=NULL WHERE id=$1")
        .bind(owner)
        .execute(&pool)
        .await
        .unwrap();
    // A verified scoped guest can register without legacy blanket approval;
    // an approved legacy profile cannot bypass a resource revocation.
    sqlx::raw_sql("CREATE SCHEMA cbgpu_accounts; CREATE TABLE cbgpu_accounts.fixture_resource(subject uuid,enabled boolean); CREATE FUNCTION cbgpu_accounts.can_user_use_resource(subject_id uuid,resource text,target_school text) RETURNS boolean LANGUAGE sql STABLE SECURITY DEFINER SET search_path='' AS $$SELECT coalesce((SELECT enabled FROM cbgpu_accounts.fixture_resource WHERE subject=subject_id AND resource='contests' AND target_school='cbsh'),false)$$;").execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO cbgpu_accounts.fixture_resource VALUES($1,true)")
        .bind(owner)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE public.profiles SET is_approved=false WHERE id=$1")
        .bind(owner)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(verifier.active(session, owner).await.unwrap(), Some(true));
    sqlx::query("UPDATE cbgpu_accounts.fixture_resource SET enabled=false")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE public.profiles SET is_approved=true WHERE id=$1")
        .bind(owner)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(verifier.active(session, owner).await.unwrap(), Some(false));
    sqlx::raw_sql("DROP SCHEMA cbgpu_accounts CASCADE")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM auth.sessions WHERE id=$1")
        .bind(session)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(verifier.active(session, owner).await.unwrap(), None);
    verifier.pool.close().await;
    sqlx::raw_sql("DELETE FROM auth.sessions;DELETE FROM public.profiles;DELETE FROM auth.users;")
        .execute(&pool)
        .await
        .unwrap();
    let bootstrap =
        std::fs::read_to_string(repo.join("schema/tests/contests/bootstrap.sql")).unwrap();
    let storage = format!(
        "create schema storage;{}",
        bootstrap.split_once("create schema storage;").unwrap().1
    );
    sqlx::raw_sql(sqlx::AssertSqlSafe(storage.as_str()))
        .execute(&pool)
        .await
        .unwrap();
    for file in [
        "schema/legacy/web-supabase/migrations/20261018051657_contest_submissions.sql",
        "schema/legacy/web-supabase/migrations/20261018052904_contest_logos.sql",
        "schema/legacy/web-supabase/migrations/20261018114551_event_management.sql",
        "schema/legacy/web-supabase/migrations/20261018120743_event_operations.sql",
        "supabase/migrations/20261116080117_contest_presentation_assets.sql",
        "supabase/migrations/20261116083445_contest_team_registration.sql",
        "supabase/migrations/20261116103228_contest_start_windows.sql",
        "schema/tests/contests/events.sql",
        "schema/tests/contests/teams.sql",
        "schema/tests/contests/start-windows.sql",
    ] {
        let sql = std::fs::read_to_string(repo.join(file))
            .unwrap()
            .lines()
            .filter(|line| !line.starts_with('\\'))
            .collect::<Vec<_>>()
            .join("\n");
        sqlx::raw_sql(sqlx::AssertSqlSafe(sql.as_str()))
            .execute(&pool)
            .await
            .unwrap_or_else(|error| panic!("Reviewed SQL contract {file}: {error}"));
    }
    pool.close().await;
}
