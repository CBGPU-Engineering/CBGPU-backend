//! Explicit private reverse snapshots. Never overwrites a legacy file or starts a controller.
use crate::{Result, fail, store::Store};
use futures_util::TryStreamExt;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{
    Postgres, Row, Transaction, TypeInfo, ValueRef,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions},
};
use std::{
    collections::BTreeMap,
    fs::{File, OpenOptions},
    io::{Read, Write},
    os::unix::{
        ffi::OsStrExt,
        fs::{MetadataExt, OpenOptionsExt},
    },
    path::{Path, PathBuf},
};
const TABLES: [&str; 7] = [
    "projects",
    "deployments",
    "custom_domains",
    "project_members",
    "server_domains",
    "compose_states",
    "work_items",
];
const RUNTIME_COLUMNS: [&str; 6] = [
    "id owner slug config secrets production created_at deleted",
    "id project_id status logs result error created_at finished_at requested_by",
    "id hostname resource_type project_id owner verification_token dns_verified tls_status provider_id provider_records last_check created_at",
    "resource_type project_id user_id role email name created_at",
    "project_id owner slug",
    "project_id requested_at status logs error result updated_at storage_gb storage_used_bytes diagnostics attempt_result access_state",
];
#[derive(Clone)]
pub struct Options {
    pub writers_fenced: bool,
    pub max_rows: u64,
    pub max_bytes: u64,
}
impl Default for Options {
    fn default() -> Self {
        Self {
            writers_fenced: false,
            max_rows: 100_000,
            max_bytes: 512 * 1024 * 1024,
        }
    }
}
impl Options {
    fn validate_bounds(&self) -> Result<()> {
        if !(1..=1_000_000).contains(&self.max_rows)
            || !(1024..=2_147_483_648).contains(&self.max_bytes)
        {
            return Err(fail("Reverse snapshot bounds invalid", 400));
        }
        Ok(())
    }
    fn validate(&self) -> Result<()> {
        self.validate_bounds()?;
        if !self.writers_fenced {
            return Err(fail(
                "Explicit stopped-writer fence acknowledgment required",
                409,
            ));
        }

        Ok(())
    }
}
#[derive(Clone)]
struct Col {
    name: String,
    kind: String,
    not_null: bool,
}
async fn schema(tx: &mut Transaction<'_, Postgres>) -> Result<BTreeMap<String, Vec<Col>>> {
    let mut result = BTreeMap::new();
    for (table, index) in TABLES.iter().zip(0..) {
        let rows=sqlx::query("SELECT column_name,data_type,is_nullable FROM information_schema.columns WHERE table_schema='deploy_store' AND table_name=$1 ORDER BY ordinal_position").bind(table).fetch_all(&mut**tx).await?;
        let cols: Vec<Col> = rows
            .into_iter()
            .map(|r| Col {
                name: r.get("column_name"),
                kind: r.get("data_type"),
                not_null: r.get::<String, _>("is_nullable") == "NO",
            })
            .collect();
        if cols.is_empty()
            || cols.iter().any(|c| {
                !matches!(
                    c.kind.as_str(),
                    "text" | "integer" | "bigint" | "boolean" | "jsonb"
                )
            })
        {
            return Err(fail("Unreviewed native snapshot schema", 409));
        }
        if index < 6 {
            let mut actual: Vec<_> = cols.iter().map(|c| c.name.as_str()).collect();
            let mut expected: Vec<_> = RUNTIME_COLUMNS[index].split_whitespace().collect();
            actual.sort();
            expected.sort();
            if actual != expected {
                return Err(fail("Legacy schema adapter review required", 409));
            }
        }
        result.insert((*table).into(), cols);
    }
    Ok(result)
}
fn json_hash(value: &Value) -> Result<String> {
    Ok(format!("{:x}", Sha256::digest(serde_json::to_vec(value)?)))
}
fn schema_value(cols: &BTreeMap<String, Vec<Col>>) -> Value {
    json!(
        cols.iter()
            .map(|(table, cols)| (
                table,
                cols.iter()
                    .map(|c| json!({"name":c.name,"type":c.kind,"not_null":c.not_null}))
                    .collect::<Vec<_>>()
            ))
            .collect::<BTreeMap<_, _>>()
    )
}
async fn lock(tx: &mut Transaction<'_, Postgres>, exclusive: bool) -> Result<()> {
    sqlx::query("SET LOCAL lock_timeout='5s'")
        .execute(&mut **tx)
        .await?;
    let mode = if exclusive {
        "ACCESS EXCLUSIVE"
    } else {
        "SHARE"
    };
    sqlx::query(sqlx::AssertSqlSafe(format!("LOCK TABLE deploy_store.projects,deploy_store.deployments,deploy_store.custom_domains,deploy_store.project_members,deploy_store.server_domains,deploy_store.compose_states,deploy_store.work_items IN {mode} MODE"))).execute(&mut**tx).await?;
    Ok(())
}
async fn drained(tx: &mut Transaction<'_, Postgres>) -> Result<()> {
    let active:i64=sqlx::query_scalar("SELECT (SELECT count(*) FROM deploy_store.work_items WHERE state IN('queued','running'))+(SELECT count(*) FROM deploy_store.deployments WHERE status IN('queued','building'))+(SELECT count(*) FROM deploy_store.compose_states WHERE status IN('queued','building'))").fetch_one(&mut**tx).await?;
    if active != 0 {
        return Err(fail(
            "Drain all native queues/builds before reverse snapshot",
            409,
        ));
    }
    Ok(())
}
fn file_hash(path: &Path) -> Result<String> {
    let mut f = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?;
    if !f.metadata()?.is_file() {
        return Err(fail("Snapshot must be a regular file", 400));
    }
    let mut hash = Sha256::new();
    let mut buf = [0; 65536];
    loop {
        let count = f.read(&mut buf)?;
        if count == 0 {
            break;
        }
        hash.update(&buf[..count]);
    }
    Ok(format!("{:x}", hash.finalize()))
}
fn sidecar(path: &Path) -> Result<PathBuf> {
    let file = path
        .file_name()
        .ok_or_else(|| fail("Snapshot filename missing", 400))?;
    if file.as_bytes().len() > 180 {
        return Err(fail("Snapshot filename too long", 400));
    }
    let mut value = file.to_os_string();
    value.push(".manifest.json");
    Ok(path.with_file_name(value))
}
struct Artifact {
    temp: PathBuf,
    destination: PathBuf,
    published: bool,
    committed: bool,
    device: u64,
    inode: u64,
}
impl Artifact {
    fn reserve(destination: &Path) -> Result<Self> {
        if !destination.is_absolute() || std::fs::symlink_metadata(destination).is_ok() {
            return Err(fail(
                "Reverse destination must be a NEW absolute file; overwrite refused",
                409,
            ));
        }
        let parent = destination
            .parent()
            .ok_or_else(|| fail("Snapshot parent missing", 400))?
            .canonicalize()?;
        let meta = std::fs::metadata(&parent)?;
        if meta.uid() != unsafe { libc::geteuid() } || meta.mode() & 0o077 != 0 {
            return Err(fail(
                "Snapshot parent must be private and owned by the operator",
                403,
            ));
        }
        let destination = parent.join(
            destination
                .file_name()
                .ok_or_else(|| fail("Snapshot filename missing", 400))?,
        );
        let temp = parent.join(format!(".cbgpu-reverse-{}.pending", uuid::Uuid::new_v4()));
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&temp)?;
        let meta = file.metadata()?;
        Ok(Self {
            temp,
            destination,
            published: false,
            committed: false,
            device: meta.dev(),
            inode: meta.ino(),
        })
    }
    fn publish(&mut self) -> Result<()> {
        File::open(&self.temp)?.sync_all()?;
        std::fs::hard_link(&self.temp, &self.destination)
            .map_err(|_| fail("Reverse destination appeared; no file overwritten", 409))?;
        self.published = true;
        Ok(())
    }
}
impl Drop for Artifact {
    fn drop(&mut self) {
        if self.published
            && !self.committed
            && std::fs::symlink_metadata(&self.destination)
                .is_ok_and(|m| m.dev() == self.device && m.ino() == self.inode)
        {
            let _ = std::fs::remove_file(&self.destination);
        }
        let _ = std::fs::remove_file(&self.temp);
    }
}
fn sqlite_ddl(table: &str, cols: &[Col]) -> String {
    let fields = cols
        .iter()
        .map(|c| {
            let kind = if matches!(c.kind.as_str(), "integer" | "bigint" | "boolean") {
                "INTEGER"
            } else {
                "TEXT"
            };
            let default = match (table, c.name.as_str()) {
                ("projects", "deleted") | ("custom_domains", "dns_verified") => " DEFAULT 0",
                ("custom_domains", "tls_status") => " DEFAULT 'pending_dns'",
                ("custom_domains", "provider_records") => " DEFAULT '[]'",
                ("deployments" | "compose_states", "logs") => " DEFAULT ''",
                ("compose_states", "storage_gb") => " DEFAULT 40",
                _ => "",
            };
            format!(
                "\"{}\" {kind}{}{default}",
                c.name,
                if c.not_null { " NOT NULL" } else { "" }
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    let tail = match table {
        "project_members" => {
            ",PRIMARY KEY(resource_type,project_id,user_id),CHECK(role IN('editor','viewer'))"
        }
        "server_domains" => ",PRIMARY KEY(project_id),UNIQUE(slug)",
        "compose_states" => ",PRIMARY KEY(project_id)",
        "projects" => ",PRIMARY KEY(id),UNIQUE(slug)",
        "deployments" => ",PRIMARY KEY(id),FOREIGN KEY(project_id)REFERENCES projects(id)",
        _ => ",PRIMARY KEY(id)",
    };
    format!("CREATE TABLE {table}({fields}{tail})")
}
fn terminal(row: &Value) -> bool {
    matches!(
        row["state"].as_str(),
        Some("succeeded" | "failed" | "cancelled")
    )
}
/// Locks a consistent native snapshot; publication is new-files-only and never changes the original SQLite.
pub async fn export_legacy(store: &Store, destination: &Path, options: &Options) -> Result<Value> {
    options.validate()?;
    let manifest_path = sidecar(destination)?;
    let mut output = Artifact::reserve(destination)?;
    let mut manifest_file = Artifact::reserve(&manifest_path)?;
    let mut tx = store.pool.begin().await?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ")
        .execute(&mut *tx)
        .await?;
    lock(&mut tx, false).await?;
    drained(&mut tx).await?;
    let columns = schema(&mut tx).await?;
    let sqlite = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(
            SqliteConnectOptions::new()
                .filename(&output.temp)
                .create_if_missing(false)
                .journal_mode(SqliteJournalMode::Memory)
                .foreign_keys(true),
        )
        .await?;
    let mut out = sqlite.begin().await?;
    let mut counts = BTreeMap::new();
    let mut archive = Vec::new();
    let (mut total_rows, mut total_bytes) = (0u64, 0u64);
    let mut fingerprint = Sha256::new();
    for table in TABLES {
        if table != "work_items" {
            sqlx::query(sqlx::AssertSqlSafe(sqlite_ddl(table, &columns[table])))
                .execute(&mut *out)
                .await?;
        }
        fingerprint.update(table.as_bytes());
        let mut count = 0u64;
        let query =
            format!("SELECT to_jsonb(t) FROM deploy_store.{table} t ORDER BY to_jsonb(t)::text");
        let mut rows =
            sqlx::query_scalar::<_, Value>(sqlx::AssertSqlSafe(query.as_str())).fetch(&mut *tx);
        while let Some(row) = rows.try_next().await? {
            let encoded = serde_json::to_vec(&row)?;
            total_rows += 1;
            total_bytes += encoded.len() as u64;
            if total_rows > options.max_rows || total_bytes > options.max_bytes {
                return Err(fail("Reverse snapshot exceeds reviewed bounds", 413));
            }
            fingerprint.update(&encoded);
            fingerprint.update(b"\n");
            if table == "work_items" {
                if !terminal(&row) {
                    return Err(fail(
                        "Active native work cannot be archived for rollback",
                        409,
                    ));
                }
                archive.push(row);
            } else {
                let cols = &columns[table];
                let mut query =
                    sqlx::QueryBuilder::<sqlx::Sqlite>::new(format!("INSERT INTO {table}("));
                query
                    .push(
                        cols.iter()
                            .map(|c| format!("\"{}\"", c.name))
                            .collect::<Vec<_>>()
                            .join(","),
                    )
                    .push(") VALUES(");
                let mut separated = query.separated(",");
                for col in cols {
                    let value = &row[&col.name];
                    if matches!(col.kind.as_str(), "integer" | "bigint") {
                        separated.push_bind(if value.is_null() {
                            None
                        } else {
                            Some(
                                value
                                    .as_i64()
                                    .ok_or_else(|| fail("Invalid integer in snapshot", 400))?,
                            )
                        });
                    } else if col.kind == "boolean" {
                        separated.push_bind(value.as_bool().map(i64::from));
                    } else {
                        let value = if value.is_null() {
                            None
                        } else if col.kind == "jsonb" {
                            Some(serde_json::to_string(value)?)
                        } else {
                            Some(
                                value
                                    .as_str()
                                    .ok_or_else(|| fail("Invalid text in snapshot", 400))?
                                    .to_owned(),
                            )
                        };
                        separated.push_bind(value);
                    }
                }
                query.push(")");
                query.build().execute(&mut *out).await?;
            }
            count += 1;
        }
        counts.insert(table, count);
    }
    sqlx::query("CREATE UNIQUE INDEX custom_domains_active_host ON custom_domains(hostname)WHERE dns_verified=1").execute(&mut*out).await?;
    out.commit().await?;
    sqlite.close().await;
    let fingerprint = format!("{:x}", fingerprint.finalize());
    tx.rollback().await?;
    if crate::importer::target_fingerprint(store).await? != fingerprint {
        return Err(fail(
            "Native state changed; reverse snapshot not published",
            409,
        ));
    }
    let manifest = json!({"format":1,"source_fingerprint":fingerprint,"schema":schema_value(&columns),"schema_sha256":json_hash(&schema_value(&columns))?,"counts":counts,"sqlite_sha256":file_hash(&output.temp)?,"native_work_items":archive,"native_data_root":store.data,"snapshot_only":true,"writer_fence":"operator-acknowledged"});
    let envelope =
        json!({"format":"cbgpu-deploy-reverse-v1","sealed_manifest":store.cipher.seal(&manifest)?});
    let bytes = serde_json::to_vec(&envelope)?;
    if bytes.len() as u64 > options.max_bytes * 2 {
        return Err(fail("Reverse manifest exceeds reviewed bounds", 413));
    }
    let mut file = OpenOptions::new()
        .write(true)
        .truncate(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&manifest_file.temp)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    output.publish()?;
    manifest_file.publish()?;
    File::open(output.destination.parent().unwrap())?.sync_all()?;
    output.committed = true;
    manifest_file.committed = true;
    Ok(
        json!({"counts":counts,"source_fingerprint":fingerprint,"ciphertext_preserved":true,"original_sqlite_overwritten":false,"workers_started":false,"native_history_archived":true}),
    )
}
fn private_bytes(path: &Path, maximum: u64) -> Result<Vec<u8>> {
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?;
    let meta = file.metadata()?;
    if !meta.is_file()
        || meta.uid() != unsafe { libc::geteuid() }
        || meta.mode() & 0o077 != 0
        || meta.len() > maximum
    {
        return Err(fail(
            "Reverse bundle must be private, regular, and within bounds",
            403,
        ));
    }
    let mut bytes = Vec::new();
    (&mut file).take(maximum + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > maximum {
        return Err(fail("Reverse bundle exceeds reviewed bounds", 413));
    }
    Ok(bytes)
}
fn read_manifest(store: &Store, source: &Path, options: &Options) -> Result<Value> {
    if !source.is_absolute() {
        return Err(fail("Reverse source must be an absolute private path", 400));
    }
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(source)?;
    let meta = file.metadata()?;
    if !meta.is_file()
        || meta.uid() != unsafe { libc::geteuid() }
        || meta.mode() & 0o077 != 0
        || meta.len() > options.max_bytes * 2
    {
        return Err(fail("Reverse SQLite privacy or size bound failed", 403));
    }
    let envelope: Value =
        serde_json::from_slice(&private_bytes(&sidecar(source)?, options.max_bytes * 2)?)?;
    if envelope["format"] != "cbgpu-deploy-reverse-v1" {
        return Err(fail("Unknown reverse bundle format", 409));
    }
    let manifest = store.cipher.unseal(
        envelope["sealed_manifest"]
            .as_str()
            .ok_or_else(|| fail("Reverse manifest seal missing", 409))?,
    )?;
    if manifest["format"] != 1
        || manifest["schema_sha256"].as_str() != Some(json_hash(&manifest["schema"])?.as_str())
        || manifest["sqlite_sha256"].as_str() != Some(file_hash(source)?.as_str())
    {
        return Err(fail(
            "Reverse snapshot hash mismatch; no import performed",
            409,
        ));
    }

    if manifest["native_data_root"] != json!(store.data) {
        return Err(fail(
            "Artifact data root differs; explicit reviewed relocation required",
            409,
        ));
    }
    Ok(manifest)
}
/// Returns only nonpersonal proof metadata, never archived rows or encrypted customer environment data.
pub fn inspect_bundle(store: &Store, source: &Path, options: &Options) -> Result<Value> {
    options.validate_bounds()?;
    let manifest = read_manifest(store, source, options)?;
    Ok(
        json!({"format":1,"counts":manifest["counts"],"source_fingerprint":manifest["source_fingerprint"],"sqlite_sha256":manifest["sqlite_sha256"],"artifact_references_preserved":true,"workers_started":false}),
    )
}
async fn tx_fingerprint(tx: &mut Transaction<'_, Postgres>) -> Result<String> {
    let mut digest = Sha256::new();
    for table in TABLES {
        digest.update(table.as_bytes());
        let query =
            format!("SELECT to_jsonb(t) FROM deploy_store.{table} t ORDER BY to_jsonb(t)::text");
        let mut rows =
            sqlx::query_scalar::<_, Value>(sqlx::AssertSqlSafe(query.as_str())).fetch(&mut **tx);
        while let Some(row) = rows.try_next().await? {
            digest.update(serde_json::to_vec(&row)?);
            digest.update(b"\n");
        }
    }
    Ok(format!("{:x}", digest.finalize()))
}
/// Authenticated private bundle restore, atomically into an EMPTY native target only.
pub async fn import_bundle(store: &Store, source: &Path, options: &Options) -> Result<Value> {
    options.validate()?;
    let manifest = read_manifest(store, source, options)?;
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
    let tables: Vec<String> = sqlx::query_scalar(
        "SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'",
    )
    .fetch_all(&mut *read)
    .await?;
    if tables.len() != 6 || tables.iter().any(|t| !TABLES[..6].contains(&t.as_str())) {
        return Err(fail("Unreviewed reverse SQLite schema", 409));
    }
    let mut tx = store.pool.begin().await?;
    lock(&mut tx, true).await?;
    let columns = schema(&mut tx).await?;
    if schema_value(&columns) != manifest["schema"] {
        return Err(fail("Reverse source/target schema proof differs", 409));
    }
    for table in TABLES {
        let count: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
            "SELECT count(*) FROM deploy_store.{table}"
        )))
        .fetch_one(&mut *tx)
        .await?;
        if count != 0 {
            return Err(fail(
                "Reverse restore refuses to overwrite existing native state",
                409,
            ));
        }
    }
    let (mut total_rows, mut total_bytes) = (0u64, 0u64);
    let mut counts = BTreeMap::new();
    for table in TABLES {
        let mut count = 0u64;
        if table == "work_items" {
            let rows = manifest["native_work_items"]
                .as_array()
                .ok_or_else(|| fail("Native history archive missing", 409))?;
            for row in rows {
                if !terminal(row) {
                    return Err(fail("Reverse bundle contains active native work", 409));
                }
                total_rows += 1;
                total_bytes += serde_json::to_vec(row)?.len() as u64;
                if total_rows > options.max_rows || total_bytes > options.max_bytes {
                    return Err(fail("Reverse restore exceeds reviewed bounds", 413));
                }
                sqlx::query("INSERT INTO deploy_store.work_items SELECT(jsonb_populate_record(NULL::deploy_store.work_items,$1)).*").bind(row).execute(&mut*tx).await?;
                count += 1;
            }
        } else {
            let actual: Vec<String> =
                sqlx::query(sqlx::AssertSqlSafe(format!("PRAGMA table_info({table})")))
                    .fetch_all(&mut *read)
                    .await?
                    .iter()
                    .map(|r| r.get("name"))
                    .collect();
            let expected: Vec<_> = columns[table].iter().map(|c| c.name.clone()).collect();
            if actual != expected {
                return Err(fail("Reverse SQLite column proof differs", 409));
            }
            let query = format!("SELECT * FROM {table}");
            let mut rows = sqlx::query(sqlx::AssertSqlSafe(query.as_str())).fetch(&mut *read);
            while let Some(row) = rows.try_next().await? {
                let mut value = json!({});
                for col in &columns[table] {
                    let raw = row.try_get_raw(col.name.as_str())?;
                    let parsed = if raw.is_null() {
                        Value::Null
                    } else if matches!(col.kind.as_str(), "integer" | "bigint") {
                        if raw.type_info().name() != "INTEGER" {
                            return Err(fail("Reverse SQLite integer type differs", 409));
                        }
                        json!(row.try_get::<i64, _>(col.name.as_str())?)
                    } else if col.kind == "boolean" {
                        let number = row.try_get::<i64, _>(col.name.as_str())?;
                        if ![0, 1].contains(&number) {
                            return Err(fail("Reverse boolean type differs", 409));
                        }
                        json!(number == 1)
                    } else {
                        let text = row.try_get::<String, _>(col.name.as_str())?;
                        if col.kind == "jsonb" {
                            serde_json::from_str(&text)?
                        } else {
                            json!(text)
                        }
                    };
                    value[&col.name] = parsed;
                }
                if table == "projects" {
                    store.cipher.unseal(
                        value["secrets"]
                            .as_str()
                            .ok_or_else(|| fail("Project ciphertext missing", 409))?,
                    )?;
                }
                total_rows += 1;
                total_bytes += serde_json::to_vec(&value)?.len() as u64;
                if total_rows > options.max_rows || total_bytes > options.max_bytes {
                    return Err(fail("Reverse restore exceeds reviewed bounds", 413));
                }
                sqlx::query(sqlx::AssertSqlSafe(format!("INSERT INTO deploy_store.{table} SELECT(jsonb_populate_record(NULL::deploy_store.{table},$1)).*"))).bind(&value).execute(&mut*tx).await?;
                count += 1;
            }
        }
        if manifest["counts"][table].as_u64() != Some(count) {
            return Err(fail("Reverse row-count proof differs", 409));
        }
        counts.insert(table, count);
    }
    drained(&mut tx).await?;
    let fingerprint = tx_fingerprint(&mut tx).await?;
    if manifest["source_fingerprint"].as_str() != Some(fingerprint.as_str())
        || manifest["sqlite_sha256"].as_str() != Some(file_hash(source)?.as_str())
    {
        return Err(fail(
            "Reverse row equality/source stability proof failed",
            409,
        ));
    }
    read.rollback().await?;
    sqlite.close().await;
    tx.commit().await?;
    Ok(
        json!({"counts":counts,"source_fingerprint":fingerprint,"full_state_parity":true,"workers_started":false,"original_sqlite_overwritten":false}),
    )
}

/// Must run with stopped writers immediately before selecting a reverse bundle for rollback.
pub async fn verify_current_bundle(
    store: &Store,
    source: &Path,
    options: &Options,
) -> Result<Value> {
    options.validate()?;
    let proof = inspect_bundle(store, source, options)?;
    if proof["source_fingerprint"].as_str()
        != Some(crate::importer::target_fingerprint(store).await?.as_str())
    {
        return Err(fail(
            "Stale reverse bundle differs from current native state",
            409,
        ));
    }
    Ok(proof)
}
