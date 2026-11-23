//! Disposable PostgreSQL only; real authority helpers and legacy write guard, synthetic records.
use serde_json::{Value, json};
use sqlx::{PgPool, postgres::PgPoolOptions};
use std::{process::Command, time::Duration};
use uuid::Uuid;
struct Database(String);
impl Drop for Database {
    fn drop(&mut self) {
        let _ = Command::new("docker")
            .args(["rm", "-f", "-v", &self.0])
            .output();
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
    let text = std::fs::read_to_string(root.join("supabase/migrations").join(file)).unwrap();
    let start = text
        .to_ascii_lowercase()
        .find(&format!("create or replace function {name}("))
        .unwrap();
    let (offset, tag) = ["$$", "$cpu$"]
        .into_iter()
        .filter_map(|tag| text[start..].find(tag).map(|offset| (offset, tag)))
        .min_by_key(|x| x.0)
        .unwrap();
    let body = start + offset + tag.len();
    let end = body + text[body..].find(&format!("{tag};")).unwrap() + tag.len() + 1;
    sql(pool, &text[start..end]).await;
}
fn user(n: u8) -> Uuid {
    format!("00000000-0000-4000-8000-{n:012}").parse().unwrap()
}
async fn call(
    pool: &PgPool,
    actor: u8,
    school: &str,
    action: &str,
    payload: Value,
) -> Result<Value, sqlx::Error> {
    let mut tx = pool.begin().await?;
    sqlx::query("SELECT set_config('request.jwt.claims',$1,true)").bind(json!({"sub":user(actor),"role":"authenticated","exp":chrono::Utc::now().timestamp()+3600,"session_id":format!("10000000-0000-4000-8000-{actor:012}")}).to_string()).execute(&mut *tx).await?;
    sqlx::query("SET LOCAL ROLE authenticated")
        .execute(&mut *tx)
        .await?;
    let result = sqlx::query_scalar("SELECT public.school_gpu_reservation_admin($1,$2,$3)")
        .bind(school)
        .bind(action)
        .bind(payload)
        .fetch_one(&mut *tx)
        .await;
    if result.is_ok() {
        tx.commit().await?;
    } else {
        tx.rollback().await?;
    }
    result
}
async fn denied(pool: &PgPool, actor: u8, school: &str, action: &str, payload: Value, code: &str) {
    let e = call(pool, actor, school, action, payload)
        .await
        .unwrap_err();
    assert_eq!(e.as_database_error().unwrap().code().as_deref(), Some(code));
}
async fn legacy(
    pool: &PgPool,
    n: u128,
    gpu: i32,
    status: &str,
    start: &str,
    end: &str,
    owner: u8,
) -> Uuid {
    let id = Uuid::from_u128(n);
    sqlx::query("INSERT INTO public.reservations(id,user_id,gpu_id,status,start_time,end_time,purpose) VALUES($1,$2,$3,$4,clock_timestamp()+$5::interval,clock_timestamp()+$6::interval,'Synthetic history')").bind(id).bind(user(owner)).bind(gpu).bind(status).bind(start).bind(end).execute(pool).await.unwrap();
    id
}
fn payload(row: &Value, action: &str) -> Value {
    json!({"reservation_id":row["id"],"source":row["source"],"action":action,"expected_status":row["status"],"expected_end_time":row["end_time"]})
}

#[tokio::test]
#[ignore = "explicit disposable PostgreSQL17; no production connections, keys or writes"]
async fn school_roles_history_actions_and_isolation() {
    let db = Database(format!(
        "cbgpu-school-admin-booking-fixture-{}",
        std::process::id()
    ));
    assert!(
        Command::new("docker")
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
                "postgres:17-alpine"
            ])
            .output()
            .unwrap()
            .status
            .success()
    );
    let address = String::from_utf8(
        Command::new("docker")
            .args(["port", &db.0, "5432/tcp"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    let url = format!("postgres://postgres@{}/postgres", address.trim());
    let mut ready = None;
    for _ in 0..80 {
        if let Ok(p) = PgPoolOptions::new().max_connections(8).connect(&url).await {
            ready = Some(p);
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let pool = ready.unwrap();
    sql(&pool, include_str!("reservation-extensions.sql")).await;
    sql(&pool,"CREATE SCHEMA extensions;CREATE EXTENSION pgcrypto WITH SCHEMA extensions;ALTER TABLE public.profiles ADD COLUMN full_name text;ALTER TABLE cbgpu_schools.memberships ADD COLUMN school_roles text[] DEFAULT '{}';ALTER TABLE cbgpu_schools.memberships ADD COLUMN student_number text;ALTER TABLE public.reservations ADD COLUMN purpose text;ALTER TABLE cbgpu_schools.gpu_resource_reservations ADD COLUMN created_at timestamptz DEFAULT now();ALTER TABLE cbgpu_schools.gpu_resources ADD COLUMN name text DEFAULT 'Synthetic GPU';CREATE TABLE public.gpus(id int PRIMARY KEY,password text);INSERT INTO public.gpus SELECT x,'unchanged-fixture-password' FROM generate_series(1,4)x;CREATE TABLE cbgpu_schools.audit(actor_id uuid,school_id text,action text,target_id uuid,before_data jsonb,after_data jsonb);ALTER TABLE cbgpu_schools.gpu_notebook_jobs ADD COLUMN status text DEFAULT 'RUNNING';ALTER TABLE cbgpu_schools.gpu_notebook_jobs ADD COLUMN updated_at timestamptz;CREATE TABLE cbgpu_schools.gpu_notebook_commands(job_id uuid,kind text,state text DEFAULT 'PENDING',UNIQUE(job_id,kind));CREATE TABLE cbgpu_schools.gpu_notebook_sessions(job_id uuid);CREATE TABLE cbgpu_schools.gpu_notebook_audit(job_id uuid,event text,detail jsonb);UPDATE cbgpu_schools.memberships SET school_roles='{GPU_ADMIN}',student_number='B-001' WHERE user_id='00000000-0000-4000-8000-000000000001' AND school_id='school-b';UPDATE cbgpu_schools.memberships SET school_roles='{SCHOOL_ADMIN}',student_number='CB-002' WHERE user_id='00000000-0000-4000-8000-000000000002' AND school_id='cbsh';UPDATE public.profiles SET full_name='Synthetic member',is_admin=(id='00000000-0000-4000-8000-000000000003')").await;
    for (file, name) in [
        (
            "20261117011230_multi_school_identity_onboarding.sql",
            "cbgpu_schools.active_member",
        ),
        (
            "20261117011230_multi_school_identity_onboarding.sql",
            "cbgpu_schools.has_role",
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
            "20261117012655_multi_school_resource_boundaries.sql",
            "public.cbgpu_personal_project_identity",
        ),
        (
            "20261118025134_school_gpu_notebook_execution.sql",
            "cbgpu_schools.gpu_notebook_permission",
        ),
        (
            "20261118025134_school_gpu_notebook_execution.sql",
            "cbgpu_schools.gpu_notebook_stop",
        ),
    ] {
        function(&pool, file, name).await;
    }
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    sql(
        &pool,
        &std::fs::read_to_string(
            root.join("schema/legacy/root-migrations/guard_gpu_reservation_overlaps.sql"),
        )
        .unwrap(),
    )
    .await;
    let migration = include_str!(
        "../../../../supabase/migrations/20261123064008_school_gpu_reservation_admin.sql"
    );
    sql(&pool, migration).await;
    sql(&pool, migration).await;
    // A platform is_admin bit alone does not grant this new school-local API.
    denied(&pool, 3, "cbsh", "list", json!({}), "42501").await;
    denied(&pool, 1, "cbsh", "list", json!({}), "42501").await;
    denied(&pool, 2, "school-b", "list", json!({}), "42501").await;
    let approved = legacy(&pool, 1, 1, "pending", "1 hour", "2 hours", 2).await;
    legacy(&pool, 2, 2, "pending", "1 hour", "2 hours", 2).await;
    legacy(&pool, 3, 3, "approved", "-1 hour", "1 hour", 2).await;
    legacy(&pool, 4, 3, "pending", "2 hours", "3 hours", 2).await;
    legacy(&pool, 5, 4, "pending", "-2 hours", "-1 hour", 2).await;
    let initial = call(&pool, 2, "cbsh", "list", json!({"limit":100}))
        .await
        .unwrap();
    assert_eq!(initial["items"].as_array().unwrap().len(), 5);
    assert!(!initial.to_string().contains("password"));
    assert!(!initial.to_string().contains("user_email"));
    let items = initial["items"].as_array().unwrap();
    let first = items
        .iter()
        .find(|r| r["id"] == approved.to_string())
        .unwrap();
    let action = payload(first, "approve");
    let result = call(&pool, 2, "cbsh", "action", action.clone())
        .await
        .unwrap();
    assert_eq!(result["reservation"]["status"], "approved");
    let rotated: String = sqlx::query_scalar("SELECT password FROM public.gpus WHERE id=1")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(rotated.len(), 32);
    denied(&pool, 2, "cbsh", "action", action, "40001").await;
    let same: String = sqlx::query_scalar("SELECT password FROM public.gpus WHERE id=1")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(rotated == same);
    let rejected = items
        .iter()
        .find(|r| r["id"] == Uuid::from_u128(2).to_string())
        .unwrap();
    call(&pool, 2, "cbsh", "action", payload(rejected, "reject"))
        .await
        .unwrap();
    let password: String = sqlx::query_scalar("SELECT password FROM public.gpus WHERE id=2")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(password, "unchanged-fixture-password");
    let busy = items
        .iter()
        .find(|r| r["id"] == Uuid::from_u128(4).to_string())
        .unwrap();
    assert_eq!(busy["allowed_actions"], json!(["reject"]));
    let e = call(&pool, 2, "cbsh", "action", payload(busy, "approve"))
        .await
        .unwrap_err();
    assert_eq!(
        e.as_database_error().unwrap().message(),
        "GPU_CURRENTLY_IN_USE"
    );
    let expired = items
        .iter()
        .find(|r| r["id"] == Uuid::from_u128(5).to_string())
        .unwrap();
    assert_eq!(expired["allowed_actions"], json!([]));
    denied(
        &pool,
        2,
        "cbsh",
        "action",
        payload(expired, "reject"),
        "40001",
    )
    .await;
    // Keyset history still reaches every original row after a newer concurrent insertion.
    let mut cursor = None;
    let mut observed = std::collections::BTreeSet::new();
    loop {
        let page = call(&pool, 2, "cbsh", "list", json!({"limit":2,"cursor":cursor}))
            .await
            .unwrap();
        for row in page["items"].as_array().unwrap() {
            assert!(observed.insert(row["id"].as_str().unwrap().to_owned()));
        }
        cursor = page["next_cursor"].as_str().map(str::to_owned);
        if observed.len() == 2 {
            legacy(&pool, 6, 4, "rejected", "1 hour", "2 hours", 2).await;
        }
        if cursor.is_none() {
            break;
        }
    }
    assert_eq!(observed.len(), 5);
    // A same-school operator sees another user's registry reservation; another school sees none.
    let registry = Uuid::from_u128(100);
    let job = Uuid::from_u128(101);
    sqlx::query("INSERT INTO cbgpu_schools.gpu_resource_reservations(id,resource_id,owner_id,starts_at,ends_at) VALUES($1,'40000000-0000-4000-8000-000000000001',$2,now()-interval '1minute',now()+interval '1hour')").bind(registry).bind(user(2)).execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO cbgpu_schools.gpu_notebook_jobs(id,reservation_id) VALUES($1,$2)")
        .bind(job)
        .bind(registry)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO cbgpu_schools.gpu_notebook_sessions VALUES($1)")
        .bind(job)
        .execute(&pool)
        .await
        .unwrap();
    let b = call(&pool, 1, "school-b", "list", json!({})).await.unwrap();
    assert_eq!(b["items"].as_array().unwrap().len(), 1);
    let booking = &b["items"][0];
    assert_eq!(booking["owner"]["id"], user(2).to_string());
    assert_eq!(booking["allowed_actions"], json!(["cancel", "revoke"]));
    denied(
        &pool,
        2,
        "cbsh",
        "action",
        payload(booking, "revoke"),
        "P0002",
    )
    .await;
    call(&pool, 1, "school-b", "action", payload(booking, "revoke"))
        .await
        .unwrap();
    let state: String =
        sqlx::query_scalar("SELECT status FROM cbgpu_schools.gpu_notebook_jobs WHERE id=$1")
            .bind(job)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(state, "STOPPING");
    let commands:i64=sqlx::query_scalar("SELECT count(*) FROM cbgpu_schools.gpu_notebook_commands WHERE job_id=$1 AND kind='STOP_NOTEBOOK'").bind(job).fetch_one(&pool).await.unwrap();
    assert_eq!(commands, 1);
    let sessions: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM cbgpu_schools.gpu_notebook_sessions WHERE job_id=$1",
    )
    .bind(job)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(sessions, 0);
    let account_admin: bool =
        sqlx::query_scalar("SELECT is_admin FROM public.profiles WHERE id=$1")
            .bind(user(1))
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(!account_admin, "school action must never promote actor");
    // A role revoked while waiting for the booking lock cannot authorize the queued action.
    let waiting_id = Uuid::from_u128(102);
    sqlx::query("INSERT INTO cbgpu_schools.gpu_resource_reservations(id,resource_id,owner_id,starts_at,ends_at) VALUES($1,'40000000-0000-4000-8000-000000000001',$2,now(),now()+interval '1hour')").bind(waiting_id).bind(user(2)).execute(&pool).await.unwrap();
    let list = call(&pool, 1, "school-b", "list", json!({})).await.unwrap();
    let waiting_row = list["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["id"] == waiting_id.to_string())
        .unwrap();
    let action = payload(waiting_row, "cancel");
    let cloned = pool.clone();
    let mut held = pool.begin().await.unwrap();
    sqlx::query("LOCK TABLE cbgpu_schools.gpu_resource_reservations IN EXCLUSIVE MODE")
        .execute(&mut *held)
        .await
        .unwrap();
    let waiting = tokio::spawn(async move { call(&cloned, 1, "school-b", "action", action).await });
    tokio::time::sleep(Duration::from_millis(100)).await;
    sql(&pool,"UPDATE cbgpu_schools.memberships SET school_roles='{}' WHERE user_id='00000000-0000-4000-8000-000000000001' AND school_id='school-b'").await;
    held.commit().await.unwrap();
    assert_eq!(
        waiting
            .await
            .unwrap()
            .unwrap_err()
            .as_database_error()
            .unwrap()
            .code()
            .as_deref(),
        Some("42501")
    );
    sql(&pool,"UPDATE cbgpu_schools.memberships SET school_roles='{GPU_ADMIN}' WHERE user_id='00000000-0000-4000-8000-000000000001' AND school_id='school-b'").await;
    sql(&pool,"UPDATE auth.sessions SET not_after=clock_timestamp()-interval '1second' WHERE user_id='00000000-0000-4000-8000-000000000001'").await;
    denied(&pool, 1, "school-b", "list", json!({}), "42501").await;
    sql(&pool,"UPDATE auth.sessions SET not_after=NULL;UPDATE cbgpu_schools.memberships SET school_roles='{}' WHERE user_id='00000000-0000-4000-8000-000000000001'").await;
    denied(&pool, 1, "school-b", "list", json!({}), "42501").await;
    denied(
        &pool,
        2,
        "cbsh",
        "list",
        json!({"cursor":"not-json"}),
        "22P02",
    )
    .await;
    denied(&pool, 2, "cbsh", "list", json!({"limit":101}), "22023").await;
    // Tables/private helpers remain inaccessible to a bare authenticated role.
    let mut tx = pool.begin().await.unwrap();
    sqlx::query("SET LOCAL ROLE authenticated")
        .execute(&mut *tx)
        .await
        .unwrap();
    assert!(
        sqlx::query("SELECT * FROM cbgpu_schools.gpu_resource_reservations")
            .fetch_all(&mut *tx)
            .await
            .is_err()
    );
    tx.rollback().await.unwrap();
    pool.close().await;
}
