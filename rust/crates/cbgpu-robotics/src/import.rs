//! Explicit, read-only SQLite snapshot import. Never run implicitly during service startup.
//! Existing device IDs, ownership, hashed credentials, leases, commands and cached missions are preserved.
use crate::{Result, db_error, fail};
use serde_json::{Map, Value, json};
use sqlx::{
    Column, PgPool, Row,
    sqlite::{SqliteConnectOptions, SqlitePoolOptions, SqliteRow},
};
use std::{collections::BTreeSet, path::Path};
const ROBOTS: &[&str] = &[
    "id",
    "owner",
    "name",
    "created",
    "revoked",
    "pair_hash",
    "pair_until",
    "token_hash",
    "agent_session",
    "seen",
    "lease",
    "lease_until",
    "telemetry",
    "map",
    "image",
    "stop_epoch",
];
const COMMANDS: &[&str] = &[
    "id",
    "device",
    "kind",
    "state",
    "created",
    "expires",
    "lease",
    "payload",
    "error",
    "updated",
    "accepted_at",
];
const PLANS: &[&str] = &[
    "id",
    "owner",
    "robot",
    "capabilities",
    "observed",
    "mission",
    "error",
];
fn invalid() -> crate::Failure {
    fail(
        "Legacy robotics snapshot has unsupported or invalid data; import rolled back",
        400,
    )
}
fn transform(table: &str, row: &SqliteRow) -> Result<Value> {
    let mut output = Map::new();
    for column in row.columns() {
        let name = column.name();
        let value = if name == "revoked" {
            match row.try_get::<i64, _>(name).map_err(|_| invalid())? {
                0 => json!(false),
                1 => json!(true),
                _ => return Err(invalid()),
            }
        } else if name == "stop_epoch" {
            json!(row.try_get::<i64, _>(name).map_err(|_| invalid())?)
        } else if [
            "created",
            "pair_until",
            "seen",
            "lease_until",
            "expires",
            "updated",
            "accepted_at",
            "observed",
        ]
        .contains(&name)
        {
            let number = row.try_get::<Option<f64>, _>(name).map_err(|_| invalid())?;
            match number {
                Some(value) if value.is_finite() => json!(value),
                Some(_) => return Err(invalid()),
                None => Value::Null,
            }
        } else {
            let text = row
                .try_get::<Option<String>, _>(name)
                .map_err(|_| invalid())?;
            match text {
                Some(text)
                    if ["telemetry", "map", "payload", "capabilities", "mission"]
                        .contains(&name) =>
                {
                    serde_json::from_str::<Value>(&text).map_err(|_| invalid())?
                }
                Some(text) => json!(text),
                None => Value::Null,
            }
        };
        output.insert(name.into(), value);
    }
    if table == "robot_commands" && !output.contains_key("accepted_at") {
        output.insert("accepted_at".into(), Value::Null);
    }
    Ok(Value::Object(output))
}
/// Requires a reviewed, quiescent read-only SQLite snapshot and an empty native robotics target.
/// All target changes and exact row/count verification occur in a single PostgreSQL transaction.
pub async fn import_read_only(pool: &PgPool, source: &Path) -> Result<Value> {
    let options = SqliteConnectOptions::new()
        .filename(source)
        .read_only(true)
        .create_if_missing(false);
    let sqlite = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .map_err(|_| fail("Read-only legacy robotics snapshot unavailable", 400))?;
    let mut source_tx = sqlite.begin().await.map_err(|_| invalid())?;
    let mut target = pool.begin().await.map_err(db_error)?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended('robotics:import',0))")
        .execute(&mut *target)
        .await
        .map_err(db_error)?;
    let mut counts = Map::new();
    for (table, columns) in [
        ("remote_robots", ROBOTS),
        ("robot_commands", COMMANDS),
        ("robot_plans", PLANS),
    ] {
        let count: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
            "SELECT count(*) FROM ai_store.{table}"
        )))
        .fetch_one(&mut *target)
        .await
        .map_err(db_error)?;
        if count != 0 {
            return Err(fail(
                "Native robotics target is not empty; import refused without changing existing data",
                409,
            ));
        }
        let exists: i64 =
            sqlx::query_scalar("SELECT count(*) FROM sqlite_master WHERE type='table' AND name=?")
                .bind(table)
                .fetch_one(&mut *source_tx)
                .await
                .map_err(|_| invalid())?;
        if exists == 0 {
            counts.insert(table.into(), json!(0));
            continue;
        }
        let description = sqlx::query(sqlx::AssertSqlSafe(format!("PRAGMA table_info({table})")))
            .fetch_all(&mut *source_tx)
            .await
            .map_err(|_| invalid())?;
        let names: BTreeSet<String> = description
            .iter()
            .map(|column| column.try_get::<String, _>("name").map_err(|_| invalid()))
            .collect::<Result<_>>()?;
        if names.iter().any(|name| !columns.contains(&name.as_str()))
            || columns.iter().any(|name| {
                !names.contains(*name) && !(*name == "accepted_at" && table == "robot_commands")
            })
        {
            return Err(invalid());
        }
        let rows = sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT * FROM {table} ORDER BY id"
        )))
        .fetch_all(&mut *source_tx)
        .await
        .map_err(|_| invalid())?;
        for row in &rows {
            let value = transform(table, row)?;
            sqlx::query(sqlx::AssertSqlSafe(format!("INSERT INTO ai_store.{table} SELECT * FROM jsonb_populate_record(NULL::ai_store.{table},$1)"))).bind(&value).execute(&mut *target).await.map_err(|_|invalid())?;
            let verified: bool = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
                "SELECT to_jsonb(r)=$1 FROM ai_store.{table} r WHERE id=$2"
            )))
            .bind(&value)
            .bind(
                value
                    .get("id")
                    .and_then(Value::as_str)
                    .ok_or_else(invalid)?,
            )
            .fetch_one(&mut *target)
            .await
            .map_err(|_| invalid())?;
            if !verified {
                return Err(fail(
                    "Robotics snapshot verification failed; import rolled back",
                    400,
                ));
            }
        }
        let actual: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
            "SELECT count(*) FROM ai_store.{table}"
        )))
        .fetch_one(&mut *target)
        .await
        .map_err(db_error)?;
        if actual != rows.len() as i64 {
            return Err(fail(
                "Robotics snapshot count mismatch; import rolled back",
                400,
            ));
        }
        counts.insert(table.into(), json!(actual));
    }
    source_tx.rollback().await.map_err(|_| invalid())?;
    sqlite.close().await;
    target.commit().await.map_err(db_error)?;
    Ok(json!({"tables":counts,"verified":true,"source_read_only":true}))
}
