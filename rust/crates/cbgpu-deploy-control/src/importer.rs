//! Explicit read-only legacy snapshot import. No implicit startup mutation.
use crate::{Result, fail, store::Store};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{
    Column, Row, TypeInfo, ValueRef,
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
};
use std::{collections::BTreeMap, path::Path};
const TABLES: &[&str] = &[
    "projects",
    "deployments",
    "custom_domains",
    "project_members",
    "server_domains",
    "compose_states",
];
fn defaults(table: &str) -> Value {
    match table {
        "projects" => json!({"production":null,"deleted":0}),
        "deployments" => {
            json!({"logs":"","result":null,"error":null,"finished_at":null,"requested_by":null})
        }
        "custom_domains" => {
            json!({"dns_verified":0,"tls_status":"pending_dns","provider_id":null,"provider_records":[],"last_check":null})
        }
        "compose_states" => {
            json!({"requested_at":null,"logs":"","error":null,"result":null,"storage_gb":40,"storage_used_bytes":null,"diagnostics":null,"attempt_result":null,"access_state":null})
        }
        _ => json!({}),
    }
}
pub async fn import_read_only(store: &Store, source: &Path) -> Result<Value> {
    let sqlite = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(
            SqliteConnectOptions::new()
                .filename(source)
                .read_only(true)
                .create_if_missing(false),
        )
        .await?;
    let mut read = sqlite.begin().await?;
    let existing: Vec<String> = sqlx::query_scalar(
        "SELECT name FROM sqlite_master WHERE type='table'AND name NOT LIKE'sqlite_%'",
    )
    .fetch_all(&mut *read)
    .await?;
    if existing
        .iter()
        .any(|table| !TABLES.contains(&table.as_str()))
    {
        return Err(fail(
            "Unknown legacy deployment table; explicit importer review required",
            409,
        ));
    }
    if !existing.iter().any(|v| v == "projects") {
        return Err(fail("Legacy deployment schema missing", 400));
    }
    for (table, condition) in [
        ("deployments", "status IN('queued','building')"),
        ("compose_states", "status='building'"),
    ] {
        if existing.iter().any(|name| name == table) {
            let active: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
                "SELECT count(*)FROM {table} WHERE {condition}"
            )))
            .fetch_one(&mut *read)
            .await?;
            if active > 0 {
                return Err(fail(
                    "Drain and stop all legacy deployment writers before importing",
                    409,
                ));
            }
        }
    }
    let mut tx = store.pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended('deploy:import',0))")
        .execute(&mut *tx)
        .await?;
    // Fence every native table for the entire import transaction, not just cooperative writers.
    sqlx::query("LOCK TABLE deploy_store.projects,deploy_store.deployments,deploy_store.custom_domains,deploy_store.project_members,deploy_store.server_domains,deploy_store.compose_states,deploy_store.work_items IN ACCESS EXCLUSIVE MODE").execute(&mut *tx).await?;
    for table in TABLES.iter().copied().chain(std::iter::once("work_items")) {
        let count: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
            "SELECT count(*)FROM deploy_store.{table}"
        )))
        .fetch_one(&mut *tx)
        .await?;
        if count != 0 {
            return Err(fail(
                "Native deployment target is not empty; overwrite refused",
                409,
            ));
        }
    }
    let mut counts = BTreeMap::new();
    let mut digest = Sha256::new();
    for table in TABLES {
        let mut count = 0usize;
        if existing.iter().any(|name| name == table) {
            let columns:Vec<String>=sqlx::query_scalar("SELECT column_name FROM information_schema.columns WHERE table_schema='deploy_store'AND table_name=$1").bind(table).fetch_all(&mut*tx).await?;
            for row in sqlx::query(sqlx::AssertSqlSafe(format!(
                "SELECT * FROM {table} ORDER BY rowid"
            )))
            .fetch_all(&mut *read)
            .await?
            {
                let mut value = defaults(table);
                for column in row.columns() {
                    let name = column.name();
                    if !columns.iter().any(|v| v == name) {
                        return Err(fail(
                            "Legacy deployment column has no native destination",
                            409,
                        ));
                    }
                    let raw = row.try_get_raw(name)?;
                    value[name] = if raw.is_null() {
                        Value::Null
                    } else {
                        match raw.type_info().name() {
                            "INTEGER" => json!(row.try_get::<i64, _>(name)?),
                            "REAL" => json!(row.try_get::<f64, _>(name)?),
                            "TEXT" => json!(row.try_get::<String, _>(name)?),
                            _ => return Err(fail("Unsupported legacy deployment value", 400)),
                        }
                    };
                }
                for field in match *table {
                    "projects" => &["config"][..],
                    "deployments" => &["result"][..],
                    "custom_domains" => &["provider_records"][..],
                    "compose_states" => &["result", "diagnostics", "attempt_result"][..],
                    _ => &[][..],
                } {
                    if let Some(text) = value[*field].as_str() {
                        value[*field] = serde_json::from_str(text)?;
                    }
                }
                if *table == "projects" {
                    store.cipher.unseal(
                        value["secrets"]
                            .as_str()
                            .ok_or_else(|| fail("Legacy project ciphertext missing", 400))?,
                    )?;
                    if !value["config"].is_object() {
                        return Err(fail("Legacy project config is invalid", 400));
                    }
                }
                sqlx::query(sqlx::AssertSqlSafe(format!("INSERT INTO deploy_store.{table} SELECT(jsonb_populate_record(NULL::deploy_store.{table},$1)).*"))).bind(&value).execute(&mut*tx).await?;
                let predicate = match *table {
                    "project_members" => "resource_type=$2 AND project_id=$3 AND user_id=$4",
                    "server_domains" | "compose_states" => "project_id=$2",
                    _ => "id=$2",
                };
                let sql =
                    format!("SELECT to_jsonb(t)=$1 FROM deploy_store.{table} t WHERE {predicate}");
                let mut query =
                    sqlx::query_scalar::<_, bool>(sqlx::AssertSqlSafe(sql.as_str())).bind(&value);
                query = if *table == "project_members" {
                    query
                        .bind(value["resource_type"].as_str())
                        .bind(value["project_id"].as_str())
                        .bind(value["user_id"].as_str())
                } else {
                    query.bind(
                        value[if ["server_domains", "compose_states"].contains(table) {
                            "project_id"
                        } else {
                            "id"
                        }]
                        .as_str(),
                    )
                };
                if !query.fetch_one(&mut *tx).await? {
                    return Err(fail("Deployment import row equality failed", 409));
                }
                digest.update(table.as_bytes());
                digest.update(serde_json::to_vec(&value)?);
                count += 1;
            }
        }
        counts.insert(*table, count);
    }
    read.rollback().await?;
    tx.commit().await?;
    sqlite.close().await;
    Ok(
        json!({"counts":counts,"verified_rows_sha256":format!("{:x}",digest.finalize()),"ciphertext_preserved":true,"key_rotated":false,"legacy_writes_performed":false}),
    )
}
/// Private normalized target fingerprint for rollback refusal after any new native writes.
pub async fn target_fingerprint(store: &Store) -> Result<String> {
    let mut tx = store.pool.begin().await?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
        .execute(&mut *tx)
        .await?;
    let mut digest = Sha256::new();
    for table in TABLES.iter().copied().chain(std::iter::once("work_items")) {
        let rows: Vec<Value> = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
            "SELECT to_jsonb(t) FROM deploy_store.{table} t ORDER BY to_jsonb(t)::text"
        )))
        .fetch_all(&mut *tx)
        .await?;
        digest.update(table.as_bytes());
        for row in rows {
            digest.update(serde_json::to_vec(&row)?);
            digest.update(b"\n");
        }
    }
    tx.rollback().await?;
    Ok(format!("{:x}", digest.finalize()))
}
pub async fn inspect_read_only(source: &Path) -> Result<Value> {
    let sqlite = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(
            SqliteConnectOptions::new()
                .filename(source)
                .read_only(true)
                .create_if_missing(false),
        )
        .await?;
    let mut tx = sqlite.begin().await?;
    let existing: Vec<String> = sqlx::query_scalar(
        "SELECT name FROM sqlite_master WHERE type='table'AND name NOT LIKE'sqlite_%'",
    )
    .fetch_all(&mut *tx)
    .await?;
    if existing.iter().any(|v| !TABLES.contains(&v.as_str())) {
        return Err(fail("Unknown legacy deployment table", 409));
    }
    if !existing.iter().any(|v| v == "projects") {
        return Err(fail("Legacy deployment schema missing", 400));
    }
    let mut counts = BTreeMap::new();
    let mut active = 0i64;
    for table in TABLES {
        if existing.iter().any(|t| t == table) {
            let count: i64 =
                sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT count(*)FROM {table}")))
                    .fetch_one(&mut *tx)
                    .await?;
            counts.insert(*table, count);
        }
    }
    for (table, predicate) in [
        ("deployments", "status IN('queued','building')"),
        ("compose_states", "status='building'"),
    ] {
        if existing.iter().any(|t| t == table) {
            active += sqlx::query_scalar::<_, i64>(sqlx::AssertSqlSafe(format!(
                "SELECT count(*)FROM {table} WHERE {predicate}"
            )))
            .fetch_one(&mut *tx)
            .await?;
        }
    }
    tx.rollback().await?;
    sqlite.close().await;
    Ok(
        json!({"counts":counts,"active_jobs":active,"quiescent":active==0,"legacy_writes_performed":false,"writer_fence_verified":false}),
    )
}
