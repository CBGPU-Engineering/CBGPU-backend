//! Read-only SQLite snapshot -> empty private PG session target. Never touches source IDs/logs.
use crate::{
    Result, SessionRow,
    crypto::{Crypto, token},
    fail,
    store::Store,
};
use rusqlite::{Connection, OpenFlags};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::path::Path;
#[derive(Serialize)]
pub struct ImportReport {
    pub sessions: usize,
    pub failures: usize,
    pub already_imported: bool,
    pub untouched_source_tables: Vec<String>,
}
#[derive(Serialize)]
struct Failure {
    key: String,
    count: i32,
    expires: i64,
}
pub async fn import_read_only(store: &Store, path: &Path, crypto: &Crypto) -> Result<ImportReport> {
    let metadata =
        std::fs::symlink_metadata(path).map_err(|_| fail("SOURCE_SNAPSHOT_UNAVAILABLE", 400))?;
    if !metadata.is_file() || metadata.len() > 256 * 1024 * 1024 {
        return Err(fail("SOURCE_MUST_BE_REGULAR_SNAPSHOT", 400));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err(fail("SOURCE_SNAPSHOT_PERMISSIONS", 400));
        }
    }
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let path = parent
        .canonicalize()
        .map_err(|_| fail("SOURCE_SNAPSHOT_UNAVAILABLE", 400))?
        .join(
            path.file_name()
                .ok_or_else(|| fail("SOURCE_SNAPSHOT_INVALID", 400))?,
        );
    let db = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .map_err(|_| fail("SOURCE_SNAPSHOT_UNAVAILABLE", 400))?;
    db.execute_batch("BEGIN")
        .map_err(|_| fail("SOURCE_SNAPSHOT_UNAVAILABLE", 400))?;
    let tables = db
        .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
        .and_then(|mut q| {
            q.query_map([], |r| r.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()
        })
        .map_err(|_| fail("SOURCE_SCHEMA_INVALID", 400))?;
    let rows=db.prepare("SELECT token_hash,csrf,expires,user_id,email,auth_version,auth_session_id,auth_tokens FROM sessions ORDER BY token_hash LIMIT 10001").and_then(|mut q|q.query_map([],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,i64>(2)?,r.get::<_,String>(3)?,r.get::<_,String>(4)?,r.get::<_,String>(5)?,r.get::<_,String>(6)?,r.get::<_,String>(7)?)))?.collect::<rusqlite::Result<Vec<_>>>()).map_err(|_|fail("SOURCE_SESSION_SCHEMA_INVALID",400))?;
    if rows.len() > 10000 {
        return Err(fail("SOURCE_REQUIRES_REVIEW", 400));
    }
    let mut sessions = Vec::new();
    for (hash, csrf, expires, user_id, email, auth_version, sid, sealed) in rows {
        if !token(&hash) || !token(&csrf) || email.len() > 254 {
            return Err(fail("SOURCE_SESSION_INVALID", 400));
        }
        let row = SessionRow {
            token_hash: hash,
            csrf,
            expires,
            user_id: user_id
                .parse()
                .map_err(|_| fail("SOURCE_SESSION_INVALID", 400))?,
            email,
            auth_version,
            auth_session_id: sid
                .parse()
                .map_err(|_| fail("SOURCE_SESSION_INVALID", 400))?,
            auth_tokens: sealed,
        };
        crypto
            .row_tokens(&row)
            .map_err(|_| fail("SOURCE_SESSION_KEY_OR_IDENTITY_MISMATCH", 400))?;
        sessions.push(row)
    }
    let failures = db
        .prepare("SELECT key,count,expires FROM failures ORDER BY key LIMIT 100001")
        .and_then(|mut q| {
            q.query_map([], |r| {
                Ok(Failure {
                    key: r.get(0)?,
                    count: r.get(1)?,
                    expires: r.get(2)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()
        })
        .map_err(|_| fail("SOURCE_FAILURE_SCHEMA_INVALID", 400))?;
    if failures.len() > 100000 || failures.iter().any(|f| !token(&f.key) || f.count < 0) {
        return Err(fail("SOURCE_REQUIRES_REVIEW", 400));
    }
    db.execute_batch("ROLLBACK")
        .map_err(|_| fail("SOURCE_SNAPSHOT_UNAVAILABLE", 400))?;
    drop(db);
    let fingerprint = format!(
        "{:x}",
        Sha256::digest(
            serde_json::to_vec(&(&sessions, &failures))
                .map_err(|_| fail("SOURCE_SNAPSHOT_INVALID", 400))?
        )
    );
    let untouched_source_tables = tables
        .into_iter()
        .filter(|t| t != "sessions" && t != "failures")
        .collect::<Vec<_>>();
    let mut tx = store
        .pool
        .begin()
        .await
        .map_err(|_| fail("PORTAL_STORE_UNAVAILABLE", 503))?;
    sqlx::query("SELECT pg_advisory_xact_lock(47281732503)")
        .execute(&mut *tx)
        .await
        .map_err(|_| fail("PORTAL_STORE_UNAVAILABLE", 503))?;
    let old: Option<String> = sqlx::query_scalar(
        "SELECT fingerprint FROM portal_auth.import_receipts WHERE scope='developer'",
    )
    .fetch_optional(&mut *tx)
    .await
    .map_err(|_| fail("PORTAL_STORE_UNAVAILABLE", 503))?;
    if let Some(old) = old {
        if old != fingerprint {
            return Err(fail("IMPORT_TARGET_ALREADY_USED", 409));
        }
        return Ok(ImportReport {
            sessions: sessions.len(),
            failures: failures.len(),
            already_imported: true,
            untouched_source_tables,
        });
    }
    let count:i64=sqlx::query_scalar("SELECT (SELECT count(*) FROM portal_auth.sessions)+(SELECT count(*) FROM portal_auth.failures)").fetch_one(&mut *tx).await.map_err(|_|fail("PORTAL_STORE_UNAVAILABLE",503))?;
    if count != 0 {
        return Err(fail("IMPORT_TARGET_MUST_BE_EMPTY", 409));
    }
    for row in &sessions {
        sqlx::query("INSERT INTO portal_auth.sessions(scope,token_hash,csrf,expires,user_id,email,auth_version,auth_session_id,auth_tokens) VALUES('developer',$1,$2,$3,$4,$5,$6,$7,$8)").bind(&row.token_hash).bind(&row.csrf).bind(row.expires).bind(row.user_id).bind(&row.email).bind(&row.auth_version).bind(row.auth_session_id).bind(&row.auth_tokens).execute(&mut *tx).await.map_err(|_|fail("IMPORT_FAILED_ROLLED_BACK",503))?;
    }
    for f in &failures {
        sqlx::query("INSERT INTO portal_auth.failures(scope,key,count,expires) VALUES('developer',$1,$2,$3)").bind(&f.key).bind(f.count).bind(f.expires).execute(&mut *tx).await.map_err(|_|fail("IMPORT_FAILED_ROLLED_BACK",503))?;
    }
    sqlx::query("INSERT INTO portal_auth.import_receipts(scope,fingerprint,session_count,failure_count) VALUES('developer',$1,$2,$3)").bind(&fingerprint).bind(sessions.len()as i64).bind(failures.len()as i64).execute(&mut *tx).await.map_err(|_|fail("IMPORT_FAILED_ROLLED_BACK",503))?;
    tx.commit()
        .await
        .map_err(|_| fail("IMPORT_FAILED_ROLLED_BACK", 503))?;
    Ok(ImportReport {
        sessions: sessions.len(),
        failures: failures.len(),
        already_imported: false,
        untouched_source_tables,
    })
}
