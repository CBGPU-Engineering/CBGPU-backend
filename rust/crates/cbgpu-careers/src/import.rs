//! Operator-only import. Read-only SQLite snapshot; PostgreSQL must be empty.
//! A production import is valid only after the legacy writer has been drained.
use serde::Serialize;
use sha2::{Digest, Sha256};
use sqlx::{
    FromRow, PgPool,
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
};
use std::path::Path;

#[derive(FromRow, Serialize)]
struct Application {
    id: String,
    token_hash: String,
    created: i64,
    role: String,
    name: String,
    email: String,
    affiliation: String,
    motivation: String,
    experience: String,
    portfolio: String,
    consent_version: String,
    status: String,
    notified: i32,
}
#[derive(FromRow, Serialize)]
struct Request {
    ip: String,
    created: i64,
}
#[derive(Serialize)]
pub struct Summary {
    pub applications: usize,
    pub requests: usize,
    pub verified: bool,
}
fn fingerprint<T: Serialize>(rows: &T) -> Result<Vec<u8>, &'static str> {
    Ok(
        Sha256::digest(serde_json::to_vec(rows).map_err(|_| "Cannot fingerprint imported data")?)
            .to_vec(),
    )
}
pub async fn sqlite(pool: &PgPool, source: &Path) -> Result<Summary, &'static str> {
    let sqlite = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(
            SqliteConnectOptions::new()
                .filename(source)
                .read_only(true)
                .create_if_missing(false),
        )
        .await
        .map_err(|_| "Cannot open the read-only SQLite source")?;
    let mut snapshot = sqlite
        .begin()
        .await
        .map_err(|_| "Cannot start source snapshot")?;
    let applications:Vec<Application>=sqlx::query_as("SELECT id,token_hash,created,role,name,email,affiliation,motivation,experience,portfolio,consent_version,status,notified FROM applications ORDER BY id").fetch_all(&mut *snapshot).await.map_err(|_|"Cannot read legacy applications")?;
    let requests: Vec<Request> =
        sqlx::query_as("SELECT ip,created FROM requests ORDER BY ip,created")
            .fetch_all(&mut *snapshot)
            .await
            .map_err(|_| "Cannot read legacy request history")?;
    let mut tx = pool
        .begin()
        .await
        .map_err(|_| "Cannot start destination transaction")?;
    sqlx::query("LOCK TABLE careers.applications,careers.requests IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *tx)
        .await
        .map_err(|_| "Cannot exclusively lock import destination")?;
    let existing:i64=sqlx::query_scalar("SELECT (SELECT count(*) FROM careers.applications)+(SELECT count(*) FROM careers.requests)").fetch_one(&mut *tx).await.map_err(|_|"Cannot inspect import destination")?;
    if existing != 0 {
        return Err("Import destination is not empty; refusing to overwrite data");
    }
    for row in &applications {
        sqlx::query("INSERT INTO careers.applications(id,token_hash,created,role,name,email,affiliation,motivation,experience,portfolio,consent_version,status,notified) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13)")
            .bind(&row.id).bind(&row.token_hash).bind(row.created).bind(&row.role).bind(&row.name).bind(&row.email).bind(&row.affiliation).bind(&row.motivation).bind(&row.experience).bind(&row.portfolio).bind(&row.consent_version).bind(&row.status).bind(row.notified).execute(&mut *tx).await.map_err(|_|"Application import failed; transaction rolled back")?;
    }
    for row in &requests {
        sqlx::query("INSERT INTO careers.requests(ip,created) VALUES($1,$2)")
            .bind(&row.ip)
            .bind(row.created)
            .execute(&mut *tx)
            .await
            .map_err(|_| "Request history import failed; transaction rolled back")?;
    }
    let imported:Vec<Application>=sqlx::query_as("SELECT id,token_hash,created,role,name,email,affiliation,motivation,experience,portfolio,consent_version,status,notified FROM careers.applications ORDER BY id").fetch_all(&mut *tx).await.map_err(|_|"Cannot verify imported applications")?;
    let imported_requests: Vec<Request> =
        sqlx::query_as("SELECT ip,created FROM careers.requests ORDER BY ip,created")
            .fetch_all(&mut *tx)
            .await
            .map_err(|_| "Cannot verify imported request history")?;
    if fingerprint(&applications)? != fingerprint(&imported)?
        || fingerprint(&requests)? != fingerprint(&imported_requests)?
    {
        return Err("Import verification failed; transaction rolled back");
    }
    let summary = Summary {
        applications: applications.len(),
        requests: requests.len(),
        verified: true,
    };
    tx.commit().await.map_err(|_| "Import commit failed")?;
    snapshot
        .rollback()
        .await
        .map_err(|_| "Cannot close read-only source snapshot")?;
    sqlite.close().await;
    Ok(summary)
}
pub async fn admin(pool: &PgPool, args: &[String]) -> Result<serde_json::Value, &'static str> {
    match args.first().map(String::as_str) {
        Some("--list") if args.len()==1 => sqlx::query_scalar("SELECT coalesce(jsonb_agg(row_to_json(a) ORDER BY created DESC),'[]'::jsonb) FROM (SELECT id,created,role,name,email,affiliation,motivation,experience,portfolio,status FROM careers.applications ORDER BY created DESC LIMIT 1000) a").fetch_one(pool).await.map_err(|_|"Cannot read applications"),
        Some("--show") if args.len()==2 || args.len()==4 => {
            if args.len()==4 {
                if args[2]!="--status" || !["received","contacted","joined","closed"].contains(&args[3].as_str()) { return Err("Invalid status command"); }
                sqlx::query("UPDATE careers.applications SET status=$1 WHERE id=$2").bind(&args[3]).bind(&args[1]).execute(pool).await.map_err(|_|"Cannot update application status")?;
            }
            Ok(sqlx::query_scalar::<_,serde_json::Value>("SELECT row_to_json(a)::jsonb FROM (SELECT id,created,role,name,email,affiliation,motivation,experience,portfolio,status,notified FROM careers.applications WHERE id=$1) a").bind(&args[1]).fetch_optional(pool).await.map_err(|_|"Cannot read application")?.unwrap_or(serde_json::Value::Null))
        }
        Some("--delete") if args.len()==2 => { sqlx::query("DELETE FROM careers.applications WHERE id=$1").bind(&args[1]).execute(pool).await.map_err(|_|"Cannot delete application")?;Ok(serde_json::json!({"deleted_if_present":true})) }
        _=>Err("Usage: --list | --show REFERENCE [--status STATUS] | --delete REFERENCE"),
    }
}
