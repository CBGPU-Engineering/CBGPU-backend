//! Real PostgreSQL transactions, real versioned permission functions and legacy write guard.
//! An explicit ignored test creates/deletes only its own disposable synthetic Docker DB.
use cbgpu_gpu_enrollment::reservations::{Caller, Extension, Scope, transact};
use chrono::{DateTime, Duration, Utc};
use sqlx::{PgPool, postgres::PgPoolOptions};
use std::{process::Command, time::Duration as Wait};
use uuid::Uuid;

struct Database(String);
impl Drop for Database {
    fn drop(&mut self) {
        let _ = Command::new("docker")
            .args(["rm", "-f", "-v", &self.0])
            .output();
    }
}
fn id(n: u128) -> Uuid {
    Uuid::from_u128(n)
}
fn caller(n: u8) -> Caller {
    Caller {
        user: format!("00000000-0000-4000-8000-{n:012}").parse().unwrap(),
        session: format!("10000000-0000-4000-8000-{n:012}").parse().unwrap(),
        expires: Utc::now().timestamp() + 3600,
    }
}
async fn sql(pool: &PgPool, text: &str) {
    sqlx::raw_sql(sqlx::AssertSqlSafe(text.to_owned()))
        .execute(pool)
        .await
        .unwrap();
}
async fn function(pool: &PgPool, file: &str, name: &str) {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let source = std::fs::read_to_string(root.join("supabase/migrations").join(file)).unwrap();
    let lower = source.to_ascii_lowercase();
    let start = lower
        .find(&format!("create or replace function {name}("))
        .unwrap();
    let body = start + source[start..].find("$$").unwrap() + 2;
    let end = body + source[body..].find("$$;").unwrap() + 3;
    sql(pool, &source[start..end]).await;
}
async fn legacy(
    pool: &PgPool,
    booking: Uuid,
    gpu: i32,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    owner: u8,
    status: &str,
) {
    sqlx::query("INSERT INTO public.reservations(id,user_id,gpu_id,status,start_time,end_time) VALUES($1,$2,$3,$4,$5,$6)").bind(booking).bind(caller(owner).user).bind(gpu).bind(status).bind(start).bind(end).execute(pool).await.unwrap();
}
fn change(old: DateTime<Utc>, new: DateTime<Utc>) -> Option<Extension> {
    Some(Extension {
        ends_at: new,
        expected_ends_at: old,
    })
}
async fn error(
    pool: &PgPool,
    actor: Caller,
    scope: Scope,
    id: Uuid,
    c: Option<Extension>,
    expected: &str,
) {
    assert_eq!(
        transact(pool, actor, scope, id, c).await.unwrap_err().1,
        expected
    );
}

#[tokio::test]
#[ignore = "explicit disposable loopback PostgreSQL fixture; no production DB"]
async fn extension_permissions_cas_and_direct_writer_races() {
    let db = Database(format!("cbgpu-extension-fixture-{}", std::process::id()));
    let output = Command::new("docker")
        .args([
            "run",
            "--name",
            &db.0,
            "--memory",
            "256m",
            "--cpus",
            "1",
            "-p",
            "127.0.0.1::5432",
            "-e",
            "POSTGRES_HOST_AUTH_METHOD=trust",
            "-d",
            "postgres:17-alpine",
        ])
        .output()
        .unwrap();
    assert!(output.status.success(), "disposable fixture startup failed");
    let port = Command::new("docker")
        .args(["port", &db.0, "5432/tcp"])
        .output()
        .unwrap();
    let address = String::from_utf8(port.stdout).unwrap();
    let url = format!("postgres://postgres@{}/postgres", address.trim());
    let mut pool = None;
    for _ in 0..80 {
        if let Ok(p) = PgPoolOptions::new().max_connections(8).connect(&url).await {
            pool = Some(p);
            break;
        }
        tokio::time::sleep(Wait::from_millis(100)).await;
    }
    let pool = pool.expect("isolated PostgreSQL ready");
    sql(&pool, include_str!("reservation-extensions.sql")).await;
    for (file, name) in [
        (
            "20261117011230_multi_school_identity_onboarding.sql",
            "cbgpu_schools.active_member",
        ),
        (
            "20261117065015_effective_school_feature_policy.sql",
            "cbgpu_schools.feature_enabled",
        ),
        (
            "20261117065015_effective_school_feature_policy.sql",
            "cbgpu_accounts.can_user_use_resource",
        ),
        (
            "20261116084450_account_access_rollover.sql",
            "cbgpu_accounts.current_session_valid",
        ),
        (
            "20261116084450_account_access_rollover.sql",
            "public.cbgpu_can_use_resource",
        ),
        (
            "20261117012655_multi_school_resource_boundaries.sql",
            "public.cbgpu_personal_project_identity",
        ),
        (
            "20261117121721_school_node_gpu_resources_reservations.sql",
            "cbgpu_schools.node_gpu_permission",
        ),
        (
            "20261118025134_school_gpu_notebook_execution.sql",
            "cbgpu_schools.gpu_notebook_permission",
        ),
    ] {
        function(&pool, file, name).await;
    }
    function(
        &pool,
        "20261118025134_school_gpu_notebook_execution.sql",
        "cbgpu_schools.gpu_resource_state",
    )
    .await;
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    sql(
        &pool,
        &std::fs::read_to_string(
            root.join("schema/legacy/root-migrations/guard_gpu_reservation_overlaps.sql"),
        )
        .unwrap(),
    )
    .await;
    // Test under a dedicated non-superuser with only the documented transaction privileges.
    sql(&pool,"REVOKE ALL ON FUNCTION public.cbgpu_personal_project_identity(),public.cbgpu_can_use_resource(text,text),cbgpu_schools.node_gpu_permission(uuid,text),cbgpu_schools.gpu_notebook_permission(uuid,text,timestamptz,timestamptz),cbgpu_schools.active_member(uuid,text),cbgpu_schools.gpu_resource_state(cbgpu_schools.gpu_resources) FROM PUBLIC,anon,authenticated,service_role").await;
    sql(
        &pool,
        include_str!("../../../../operations/gpu-reservations/activation.sql"),
    )
    .await;
    sql(&pool,"ALTER ROLE cbgpu_gpu_reservations LOGIN CONNECTION LIMIT 8 VALID UNTIL '2099-01-01' PASSWORD 'synthetic-fixture-only';ALTER ROLE cbgpu_gpu_reservations SET default_transaction_isolation='repeatable read'").await;
    const ROLE_STATE: &str = "SELECT md5(rolpassword||rolcanlogin::text||rolconnlimit::text||rolvaliduntil::text||(SELECT setconfig::text FROM pg_db_role_setting WHERE setrole=pg_authid.oid AND setdatabase=0)) FROM pg_authid WHERE rolname='cbgpu_gpu_reservations'";
    let before: String = sqlx::query_scalar(ROLE_STATE)
        .fetch_one(&pool)
        .await
        .unwrap();
    sql(
        &pool,
        include_str!("../../../../operations/gpu-reservations/activation.sql"),
    )
    .await;
    let after: String = sqlx::query_scalar(ROLE_STATE)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(
        before == after,
        "rerun must preserve LOGIN, password, expiry, connection limit and default settings"
    );
    for (make_bad, restore) in [
        (
            "ALTER ROLE cbgpu_gpu_reservations INHERIT",
            "ALTER ROLE cbgpu_gpu_reservations NOINHERIT",
        ),
        (
            "ALTER ROLE cbgpu_gpu_reservations CREATEDB",
            "ALTER ROLE cbgpu_gpu_reservations NOCREATEDB",
        ),
        (
            "CREATE ROLE fixture_unexpected_parent;GRANT fixture_unexpected_parent TO cbgpu_gpu_reservations",
            "REVOKE fixture_unexpected_parent FROM cbgpu_gpu_reservations;DROP ROLE fixture_unexpected_parent",
        ),
    ] {
        sql(&pool, make_bad).await;
        let mut connection = pool.acquire().await.unwrap();
        let error = sqlx::raw_sql(include_str!(
            "../../../../operations/gpu-reservations/activation.sql"
        ))
        .execute(&mut *connection)
        .await
        .unwrap_err();
        assert!(
            error
                .as_database_error()
                .unwrap()
                .message()
                .contains("differs from reviewed privilege boundary")
        );
        sqlx::query("ROLLBACK")
            .execute(&mut *connection)
            .await
            .unwrap();
        sql(&pool, restore).await;
    }
    let rolepool = PgPoolOptions::new()
        .max_connections(8)
        .connect(&url.replace("postgres@", "cbgpu_gpu_reservations@"))
        .await
        .unwrap();
    let mut lock_probe = rolepool.begin().await.unwrap();
    sqlx::query("LOCK TABLE public.reservations IN EXCLUSIVE MODE")
        .execute(&mut *lock_probe)
        .await
        .unwrap();
    lock_probe.rollback().await.unwrap();
    let now = DateTime::from_timestamp_micros(Utc::now().timestamp_micros()).unwrap();
    let start = now - Duration::minutes(10);
    let end = now + Duration::minutes(30);
    legacy(&pool, id(1), 1, start, end, 1, "approved").await;
    let preview = transact(&rolepool, caller(1), Scope::Legacy, id(1), None)
        .await
        .unwrap();
    assert_eq!(preview["can_extend"], true);
    assert!(preview["max_end"].is_null());
    let cleared: bool =
        sqlx::query_scalar("SELECT nullif(current_setting('request.jwt.claims',true),'') IS NULL")
            .fetch_one(&rolepool)
            .await
            .unwrap();
    assert!(
        cleared,
        "caller/service claims must not leak through pooled connections"
    );
    error(
        &rolepool,
        caller(2),
        Scope::Legacy,
        id(1),
        change(end, end + Duration::minutes(5)),
        "RESERVATION_NOT_FOUND",
    )
    .await;
    let updated = transact(
        &rolepool,
        caller(1),
        Scope::Legacy,
        id(1),
        change(end, end + Duration::minutes(5)),
    )
    .await
    .unwrap();
    assert_eq!(updated["updated"], true);
    let same: (Uuid, DateTime<Utc>, String) = sqlx::query_as(
        "SELECT user_id,start_time,ssh_password FROM public.reservations WHERE id=$1",
    )
    .bind(id(1))
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        same,
        (caller(1).user, start, "synthetic-password".to_owned())
    );
    error(
        &rolepool,
        caller(1),
        Scope::Legacy,
        id(1),
        change(end, end + Duration::minutes(10)),
        "RESERVATION_CHANGED",
    )
    .await;
    assert!(
        sqlx::query("UPDATE public.reservations SET ssh_password='changed'")
            .execute(&rolepool)
            .await
            .is_err()
    );
    // Pending bookings are authoritative, including exact half-open adjacency.
    let current = end + Duration::minutes(5);
    let boundary = end + Duration::minutes(20);
    legacy(
        &pool,
        id(2),
        1,
        boundary,
        boundary + Duration::minutes(20),
        2,
        "pending",
    )
    .await;
    error(
        &rolepool,
        caller(1),
        Scope::Legacy,
        id(1),
        change(current, boundary + Duration::seconds(1)),
        "RESERVATION_CONFLICT",
    )
    .await;
    transact(
        &rolepool,
        caller(1),
        Scope::Legacy,
        id(1),
        change(current, boundary),
    )
    .await
    .unwrap();
    // Expired/future/pending reservations cannot be extended or resurrected.
    for (n, s, e, status, reason) in [
        (
            3,
            start,
            now - Duration::minutes(1),
            "approved",
            "RESERVATION_EXPIRED",
        ),
        (
            4,
            now + Duration::hours(1),
            now + Duration::hours(2),
            "approved",
            "RESERVATION_NOT_ACTIVE",
        ),
        (5, start, end, "pending", "RESERVATION_NOT_ACTIVE"),
    ] {
        legacy(&pool, id(n), n as i32, s, e, 1, status).await;
        error(
            &rolepool,
            caller(1),
            Scope::Legacy,
            id(n),
            change(e, e + Duration::minutes(1)),
            reason,
        )
        .await;
    }
    sql(&pool,"UPDATE cbgpu_schools.memberships SET status='SUSPENDED' WHERE user_id='00000000-0000-4000-8000-000000000001'").await;
    error(
        &rolepool,
        caller(1),
        Scope::Legacy,
        id(1),
        None,
        "GPU_PERMISSION_REQUIRED",
    )
    .await;
    sql(&pool,"UPDATE cbgpu_schools.memberships SET status='ACTIVE' WHERE user_id='00000000-0000-4000-8000-000000000001';UPDATE auth.sessions SET not_after=now()-interval '1minute' WHERE user_id='00000000-0000-4000-8000-000000000001'").await;
    error(
        &rolepool,
        caller(1),
        Scope::Legacy,
        id(1),
        None,
        "INVALID_SESSION",
    )
    .await;
    sql(&pool,"UPDATE auth.sessions SET not_after=NULL;UPDATE public.profiles SET account_status='SUSPENDED' WHERE id='00000000-0000-4000-8000-000000000001'").await;
    error(
        &rolepool,
        caller(1),
        Scope::Legacy,
        id(1),
        None,
        "INVALID_SESSION",
    )
    .await;
    sql(&pool, "UPDATE public.profiles SET account_status='ACTIVE'").await;
    // Guest permission must cover the entire remaining interval and fresh sponsor status.
    legacy(&pool, id(6), 6, start, end, 3, "approved").await;
    sqlx::query(
        "INSERT INTO cbgpu_accounts.guest_requests VALUES($1,$2,'cbsh','APPROVED','{gpu}',$3,$4)",
    )
    .bind(caller(3).user)
    .bind(caller(2).user)
    .bind(start)
    .bind(end + Duration::minutes(10))
    .execute(&pool)
    .await
    .unwrap();
    error(
        &rolepool,
        caller(3),
        Scope::Legacy,
        id(6),
        change(end, end + Duration::minutes(11)),
        "EXTENSION_LIMIT",
    )
    .await;
    transact(
        &rolepool,
        caller(3),
        Scope::Legacy,
        id(6),
        change(end, end + Duration::minutes(5)),
    )
    .await
    .unwrap();
    sql(&pool,"UPDATE cbgpu_schools.memberships SET status='SUSPENDED' WHERE user_id='00000000-0000-4000-8000-000000000002'").await;
    error(
        &rolepool,
        caller(3),
        Scope::Legacy,
        id(6),
        None,
        "GPU_PERMISSION_REQUIRED",
    )
    .await;
    sql(
        &pool,
        "UPDATE cbgpu_schools.memberships SET status='ACTIVE'",
    )
    .await;
    // A new direct pending booking gets the lock first; extension must see its committed row.
    legacy(&pool, id(7), 7, start, end, 1, "approved").await;
    let mut direct = pool.begin().await.unwrap();
    sqlx::query("INSERT INTO public.reservations(id,user_id,gpu_id,status,start_time,end_time) VALUES($1,$2,7,'pending',$3,$4)").bind(id(8)).bind(caller(2).user).bind(end+Duration::minutes(5)).bind(end+Duration::minutes(15)).execute(&mut *direct).await.unwrap();
    let cloned = rolepool.clone();
    let task = tokio::spawn(async move {
        transact(
            &cloned,
            caller(1),
            Scope::Legacy,
            id(7),
            change(end, end + Duration::minutes(10)),
        )
        .await
    });
    tokio::time::sleep(Wait::from_millis(100)).await;
    assert!(!task.is_finished());
    direct.commit().await.unwrap();
    assert_eq!(task.await.unwrap().unwrap_err().1, "RESERVATION_CONFLICT");
    // The opposite ordering: extension holds the lock, then a direct INSERT waits
    // and must reject against the newly committed longer interval.
    legacy(&pool, id(11), 11, start, end, 1, "approved").await;
    sql(&pool,"CREATE FUNCTION fixture_extension_pause() RETURNS trigger LANGUAGE plpgsql AS $$BEGIN IF NEW.gpu_id=11 THEN PERFORM pg_sleep(0.3);END IF;RETURN NEW;END$$; CREATE TRIGGER zz_fixture_extension_pause AFTER UPDATE ON public.reservations FOR EACH ROW EXECUTE FUNCTION fixture_extension_pause()").await;
    let cloned = rolepool.clone();
    let extension = tokio::spawn(async move {
        transact(
            &cloned,
            caller(1),
            Scope::Legacy,
            id(11),
            change(end, end + Duration::minutes(10)),
        )
        .await
    });
    let mut locked = false;
    for _ in 0..100 {
        locked=sqlx::query_scalar("SELECT EXISTS(SELECT FROM pg_locks WHERE relation='public.reservations'::regclass AND mode='ExclusiveLock' AND granted)").fetch_one(&pool).await.unwrap();
        if locked {
            break;
        }
        tokio::time::sleep(Wait::from_millis(5)).await;
    }
    assert!(locked);
    let insertion=sqlx::query("INSERT INTO public.reservations(id,user_id,gpu_id,status,start_time,end_time) VALUES($1,$2,11,'pending',$3,$4)").bind(id(12)).bind(caller(2).user).bind(end+Duration::minutes(5)).bind(end+Duration::minutes(15)).execute(&pool).await.unwrap_err();
    assert_eq!(
        insertion.as_database_error().unwrap().code().as_deref(),
        Some("23P01")
    );
    assert!(extension.await.unwrap().is_ok());
    sql(&pool,"DROP TRIGGER zz_fixture_extension_pause ON public.reservations;DROP FUNCTION fixture_extension_pause()").await;
    // Concurrent extensions use a compare-and-swap; exactly one succeeds.
    let (a, b) = tokio::join!(
        transact(
            &rolepool,
            caller(1),
            Scope::Legacy,
            id(7),
            change(end, end + Duration::minutes(1))
        ),
        transact(
            &rolepool,
            caller(1),
            Scope::Legacy,
            id(7),
            change(end, end + Duration::minutes(2))
        )
    );
    assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
    // Reservation-only school extension obeys the original 4h cap; jobs remain untouched.
    sqlx::query("INSERT INTO cbgpu_schools.gpu_resource_reservations(id,resource_id,owner_id,starts_at,ends_at) VALUES($1,'40000000-0000-4000-8000-000000000001',$2,$3,$4)").bind(id(9)).bind(caller(1).user).bind(start).bind(end).execute(&pool).await.unwrap();
    let school = Scope::School("school-b".to_owned());
    error(
        &rolepool,
        caller(1),
        school.clone(),
        id(9),
        change(end, start + Duration::hours(4) + Duration::seconds(1)),
        "EXTENSION_LIMIT",
    )
    .await;
    transact(
        &rolepool,
        caller(1),
        school.clone(),
        id(9),
        change(end, end + Duration::minutes(5)),
    )
    .await
    .unwrap();
    for state in ["OFFLINE", "REVOKED"] {
        sqlx::query("UPDATE cbgpu_schools.gpu_nodes SET status=$1")
            .bind(state)
            .execute(&pool)
            .await
            .unwrap();
        error(
            &rolepool,
            caller(1),
            school.clone(),
            id(9),
            change(end + Duration::minutes(5), end + Duration::minutes(10)),
            "GPU_RESOURCE_UNAVAILABLE",
        )
        .await;
    }
    sql(&pool, "UPDATE cbgpu_schools.gpu_nodes SET status='ONLINE'").await;
    sqlx::query("INSERT INTO cbgpu_schools.gpu_notebook_jobs VALUES($1,$2)")
        .bind(id(10))
        .bind(id(9))
        .execute(&pool)
        .await
        .unwrap();
    error(
        &rolepool,
        caller(1),
        school.clone(),
        id(9),
        change(end + Duration::minutes(5), end + Duration::minutes(10)),
        "EXECUTOR_EXTENSION_UNSUPPORTED",
    )
    .await;
    let preview = transact(&rolepool, caller(1), school, id(9), None)
        .await
        .unwrap();
    assert_eq!(preview["reason"], "EXECUTOR_EXTENSION_UNSUPPORTED");
    // Revocation committed while waiting for the booking lock is rechecked.
    let mut held = pool.begin().await.unwrap();
    sqlx::query("LOCK TABLE public.reservations IN EXCLUSIVE MODE")
        .execute(&mut *held)
        .await
        .unwrap();
    let cloned = rolepool.clone();
    let waiting = tokio::spawn(async move {
        transact(
            &cloned,
            caller(1),
            Scope::Legacy,
            id(11),
            change(end + Duration::minutes(10), end + Duration::minutes(11)),
        )
        .await
    });
    tokio::time::sleep(Wait::from_millis(100)).await;
    sql(&pool,"UPDATE cbgpu_schools.memberships SET status='SUSPENDED' WHERE user_id='00000000-0000-4000-8000-000000000001'").await;
    held.commit().await.unwrap();
    assert_eq!(
        waiting.await.unwrap().unwrap_err().1,
        "GPU_PERMISSION_REQUIRED"
    );
    sql(
        &pool,
        "UPDATE cbgpu_schools.memberships SET status='ACTIVE'",
    )
    .await;
    // Bounded lock wait fails closed; no deadline update occurs.
    let mut held = pool.begin().await.unwrap();
    sqlx::query("LOCK TABLE public.reservations IN EXCLUSIVE MODE")
        .execute(&mut *held)
        .await
        .unwrap();
    error(
        &rolepool,
        caller(1),
        Scope::Legacy,
        id(7),
        change(end, end + Duration::minutes(3)),
        "RESERVATION_BUSY",
    )
    .await;
    held.rollback().await.unwrap();
    rolepool.close().await;
    pool.close().await;
}
