//! Explicit read-only SQLite handoff. Never invoked by normal server startup.
use crate::{Result, Store, fail, read_json};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{
    Column, Row, TypeInfo, ValueRef,
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
};
use std::{collections::BTreeMap, path::Path};
pub async fn import_read_only(store: &Store, source: &Path) -> Result<Value> {
    if !source.is_file() {
        return Err(fail("Source SQLite file missing", 400));
    }
    let legacy = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(
            SqliteConnectOptions::new()
                .filename(source)
                .read_only(true)
                .create_if_missing(false),
        )
        .await?;
    let mut read = legacy.begin().await?;
    let mut tx = store.pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended('cbgpu:ai:import',0))")
        .execute(&mut *tx)
        .await?;
    let tables = [
        "ai_projects",
        "uploads",
        "jobs",
        "ai_project_files",
        "api_keys",
        "downloads",
        "vectors",
        "conversations",
        "chat_messages",
    ];
    for table in tables {
        let count: i64 = sqlx::query_scalar(&format!("SELECT count(*) FROM ai_store.{table}"))
            .fetch_one(&mut *tx)
            .await?;
        if count != 0 {
            return Err(fail(
                "AI import target is not empty; refusing overwrite",
                409,
            ));
        }
    }
    let existing: Vec<String> = sqlx::query_scalar(
        "SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'",
    )
    .fetch_all(&mut *read)
    .await?;
    let skipped = existing
        .iter()
        .filter(|n| !tables.contains(&n.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    let mut counts = BTreeMap::new();
    let mut digest = Sha256::new();
    for table in tables {
        if !existing.iter().any(|t| t == table) {
            counts.insert(table, 0usize);
            continue;
        }
        let rows = sqlx::query(&format!("SELECT * FROM {table} ORDER BY rowid"))
            .fetch_all(&mut *read)
            .await?;
        let allowed:Vec<String>=sqlx::query_scalar("SELECT column_name FROM information_schema.columns WHERE table_schema='ai_store' AND table_name=$1").bind(table).fetch_all(&mut *tx).await?;
        for row in &rows {
            let mut data = defaults(table);
            for column in row.columns() {
                let name = column.name();
                if !allowed.iter().any(|c| c == name) {
                    return Err(fail(
                        "Legacy AI column has no PostgreSQL destination; refusing data loss",
                        409,
                    ));
                }
                let raw = row.try_get_raw(name)?;
                let value = if raw.is_null() {
                    Value::Null
                } else {
                    match raw.type_info().name() {
                        "INTEGER" => json!(row.try_get::<i64, _>(name)?),
                        "REAL" => json!(row.try_get::<f64, _>(name)?),
                        "TEXT" => json!(row.try_get::<String, _>(name)?),
                        _ => return Err(fail("Unsupported legacy SQLite value", 400)),
                    }
                };
                data[name] = value;
            }
            for field in match table {
                "vectors" => &["vector"][..],
                "chat_messages" => &["web_sources"][..],
                _ => &[][..],
            } {
                if let Some(text) = data[*field].as_str() {
                    data[*field] = serde_json::from_str(text)?;
                }
            }
            if table == "jobs" {
                let job = data["id"]
                    .as_str()
                    .ok_or_else(|| fail("Invalid legacy job", 400))?;
                if !crate::valid_id(job) {
                    return Err(fail("Invalid legacy job ID", 400));
                }
                let input = store.root.join("jobs").join(job).join("input.json");
                if input.is_file() {
                    data["input"] = read_json(&input)?;
                }
            }
            // Explicit destination composite forbids accidentally importing unknown columns.
            digest.update(table.as_bytes());
            digest.update(serde_json::to_vec(&data)?);
            sqlx::query(&format!("INSERT INTO ai_store.{table} SELECT (jsonb_populate_record(NULL::ai_store.{table},$1)).*")).bind(&data).execute(&mut *tx).await?;
            let key = match table {
                "vectors" => "hash",
                "ai_project_files" => "upload",
                _ => "id",
            };
            let preserved: bool = sqlx::query_scalar(&format!(
                "SELECT to_jsonb(t)=$1 FROM ai_store.{table} t WHERE {key}=$2"
            ))
            .bind(&data)
            .bind(
                data[key]
                    .as_str()
                    .ok_or_else(|| fail("Invalid legacy row identity", 400))?,
            )
            .fetch_one(&mut *tx)
            .await?;
            if !preserved {
                return Err(fail(
                    "AI import field verification failed; transaction rolled back",
                    503,
                ));
            }
        }
        let count: i64 = sqlx::query_scalar(&format!("SELECT count(*) FROM ai_store.{table}"))
            .fetch_one(&mut *tx)
            .await?;
        if count != rows.len() as i64 {
            return Err(fail("AI import row verification failed", 503));
        }
        counts.insert(table, rows.len());
    }
    read.rollback().await?;
    legacy.close().await;
    tx.commit().await?;
    Ok(
        json!({"ok":true,"counts":counts,"snapshot_sha256":digest.finalize().iter().map(|b|format!("{b:02x}")).collect::<String>(),"unhandled_tables":skipped,"source_read_only":true,"files_moved":false}),
    )
}
fn defaults(table: &str) -> Value {
    match table {
        "jobs" => {
            json!({"gpu":null,"attempts":0,"progress":"","error":null,"started":null,"finished":null,"runtime":0.,"crashes":0,"input":{}})
        }
        "uploads" => json!({"job_id":null}),
        "api_keys" => json!({"last_used":null,"revoked":0}),
        "downloads" => json!({"path":null}),
        "conversations" => {
            json!({"model":"cbgpu-chat","effort":"medium","project":null,"site_search":0})
        }
        "chat_messages" => json!({"job_id":null,"web_sources":[]}),
        _ => json!({}),
    }
}
