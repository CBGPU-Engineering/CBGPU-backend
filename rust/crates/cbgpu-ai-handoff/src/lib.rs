//! Explicit private AI handoff. Preflight never mutates a database or service.
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    time::Duration,
};
pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
const TABLES: &[&str] = &[
    "jobs",
    "uploads",
    "ai_projects",
    "ai_project_files",
    "api_keys",
    "downloads",
    "vectors",
    "conversations",
    "chat_messages",
    "remote_robots",
    "robot_commands",
    "robot_plans",
    "robot_sync_events",
];
#[derive(Debug)]
pub struct Inspection {
    pub counts: BTreeMap<String, i64>,
    pub active: i64,
}
pub async fn inspect(source: &Path) -> Result<Inspection> {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(
            SqliteConnectOptions::new()
                .filename(source)
                .read_only(true)
                .create_if_missing(false),
        )
        .await?;
    let mut tx = pool.begin().await?;
    let available: Vec<String> = sqlx::query_scalar(
        "SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'",
    )
    .fetch_all(&mut *tx)
    .await?;
    if available
        .iter()
        .any(|name| !TABLES.contains(&name.as_str()))
    {
        return Err("Unknown legacy tables require an explicitly reviewed importer".into());
    }
    let mut counts = BTreeMap::new();
    for table in TABLES {
        if available.iter().any(|name| name == table) {
            let n: i64 = sqlx::query_scalar(&format!("SELECT count(*) FROM {table}"))
                .fetch_one(&mut *tx)
                .await?;
            counts.insert((*table).into(), n);
        }
    }
    if !available.iter().any(|n| n == "jobs") {
        return Err("Legacy jobs table is missing".into());
    }
    let active:i64=sqlx::query_scalar("SELECT count(*) FROM jobs WHERE state NOT IN('succeeded','failed','cancelled','deleted') OR state IS NULL").fetch_one(&mut *tx).await?;
    tx.rollback().await?;
    pool.close().await;
    Ok(Inspection { counts, active })
}
fn unit_name(name: &str) -> Result<()> {
    if !name.ends_with(".service")
        || name.is_empty()
        || !name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.@".contains(&b))
    {
        return Err("Invalid explicit service unit name".into());
    }
    Ok(())
}
pub async fn unit_state(name: &str) -> Result<Value> {
    unit_state_with_tool(name, Path::new("systemctl")).await
}
async fn unit_state_with_tool(name: &str, tool: &Path) -> Result<Value> {
    unit_name(name)?;
    let output = tokio::time::timeout(
        Duration::from_secs(8),
        tokio::process::Command::new(tool)
            .args([
                "--user",
                "show",
                name,
                "--property=ActiveState,SubState,UnitFileState,MainPID",
                "--no-pager",
            ])
            .kill_on_drop(true)
            .output(),
    )
    .await??;
    if !output.status.success() {
        return Err("Cannot establish user service state".into());
    }
    let fields: BTreeMap<_, _> = String::from_utf8(output.stdout)?
        .lines()
        .filter_map(|line| {
            line.split_once('=')
                .map(|(k, v)| (k.to_owned(), v.to_owned()))
        })
        .collect();
    Ok(json!(fields))
}
pub fn stopped_barrier(legacy: &Value, native: &Value) -> Result<()> {
    for state in [legacy, native] {
        if state["ActiveState"] != "inactive" || state["MainPID"] != "0" {
            return Err(
                "Both legacy and native AI services must be inactive with no main process".into(),
            );
        }
    }
    if !matches!(
        legacy["UnitFileState"].as_str(),
        Some("masked" | "masked-runtime")
    ) {
        return Err(
            "Legacy AI unit must be explicitly runtime-masked to prevent concurrent writers".into(),
        );
    }
    Ok(())
}
async fn ports_idle(ports: &[u16]) -> Result<()> {
    for port in ports {
        match tokio::time::timeout(
            Duration::from_millis(300),
            tokio::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, *port)),
        )
        .await
        {
            Ok(Ok(_)) => {
                return Err(
                    "An AI listener is still accepting traffic; stop all writers before import"
                        .into(),
                );
            }
            Ok(Err(error)) if error.kind() == std::io::ErrorKind::ConnectionRefused => {}
            _ => return Err("Cannot establish that AI listeners are stopped".into()),
        }
    }
    Ok(())
}
pub async fn preflight(state: &Path, legacy: &str, native: &str) -> Result<Value> {
    let inspection = inspect(&state.join("queue.sqlite")).await?;
    let old = unit_state(legacy).await?;
    let new = unit_state(native).await?;
    Ok(
        json!({"mode":"read-only","counts":inspection.counts,"active_jobs":inspection.active,"drained":inspection.active==0,"legacy_unit":old,"native_unit":new,"barrier_ready":stopped_barrier(&old,&new).is_ok(),"routing":{"legacy":"127.0.0.1:3550","native_candidate_default":"127.0.0.1:13550","required_cutover_listen":"127.0.0.1:3550"},"activation_performed":false}),
    )
}
fn private_dir(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::create_dir(path)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    Ok(())
}
fn private_json(path: &Path, value: &Value) -> Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(path)?;
    serde_json::to_writer(&mut file, value)?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    Ok(())
}
fn file_digest(path: &Path) -> Result<String> {
    let mut file = File::open(path)?;
    let mut sha = Sha256::new();
    let mut buf = [0; 65536];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        sha.update(&buf[..n])
    }
    Ok(format!("{:x}", sha.finalize()))
}
/// Copy every regular file and preserve links; never follow a source link into a loop.
/// State and bulk trees are backed up separately so bulk upload links remain restorable.
fn copy_tree(source: &Path, dest: &Path) -> Result<()> {
    use std::os::unix::fs::{PermissionsExt, symlink};
    private_dir(dest)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let target = dest.join(entry.file_name());
        if ty.is_dir() {
            copy_tree(&entry.path(), &target)?
        } else if ty.is_symlink() {
            symlink(fs::read_link(entry.path())?, &target)?
        } else if ty.is_file() {
            fs::copy(entry.path(), &target)?;
            let owner_execute = fs::metadata(entry.path())?.permissions().mode() & 0o100;
            fs::set_permissions(&target, fs::Permissions::from_mode(0o600 | owner_execute))?;
            File::open(&target)?.sync_all()?;
            if file_digest(&entry.path())? != file_digest(&target)? {
                return Err("Snapshot file equality verification failed".into());
            }
        } else {
            return Err("Snapshot refuses special files".into());
        }
    }
    File::open(dest)?.sync_all()?;
    Ok(())
}
fn tree_bytes(path: &Path) -> Result<u64> {
    let mut bytes = 0u64;
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let meta = fs::symlink_metadata(entry.path())?;
        let n = if meta.is_dir() {
            tree_bytes(&entry.path())?
        } else if meta.is_file() {
            meta.len()
        } else {
            0
        };
        bytes = bytes.checked_add(n).ok_or("Snapshot size overflow")?;
    }
    Ok(bytes)
}
fn ensure_capacity(parent: &Path, bytes: u64) -> Result<()> {
    use std::os::unix::ffi::OsStrExt;
    let name = std::ffi::CString::new(parent.as_os_str().as_bytes())?;
    let mut stats = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    if unsafe { libc::statvfs(name.as_ptr(), stats.as_mut_ptr()) } != 0 {
        return Err("Cannot establish snapshot disk capacity".into());
    }
    let stats = unsafe { stats.assume_init() };
    let free = (stats.f_bavail as u128) * (stats.f_frsize as u128);
    if free < (bytes as u128) + 64 * 1024 * 1024 {
        return Err("Insufficient disk space for a fully copied private snapshot".into());
    }
    Ok(())
}
fn source_fingerprint(state: &Path) -> Result<Value> {
    let mut values = BTreeMap::new();
    for suffix in ["queue.sqlite", "queue.sqlite-wal"] {
        let p = state.join(suffix);
        if p.exists() {
            values.insert(suffix, file_digest(&p)?);
        }
    }
    Ok(json!(values))
}
async fn target_fingerprint(pool: &sqlx::PgPool) -> Result<Value> {
    let mut rows = BTreeMap::new();
    for table in TABLES {
        let present: bool = sqlx::query_scalar("SELECT to_regclass($1) IS NOT NULL")
            .bind(format!("ai_store.{table}"))
            .fetch_one(pool)
            .await?;
        if !present {
            continue;
        }
        let mut digest = Sha256::new();
        let values: Vec<String> = sqlx::query_scalar(&format!(
            "SELECT to_jsonb(t)::text FROM ai_store.{table} t ORDER BY to_jsonb(t)::text"
        ))
        .fetch_all(pool)
        .await?;
        for value in &values {
            digest.update((value.len() as u64).to_be_bytes());
            digest.update(value.as_bytes())
        }
        rows.insert(
            (*table).to_owned(),
            json!({"count":values.len(),"sha256":format!("{:x}",digest.finalize())}),
        );
    }
    Ok(json!(rows))
}
pub struct ImportOptions<'a> {
    pub state: &'a Path,
    pub bulk: Option<&'a Path>,
    pub backup: &'a Path,
    pub database: &'a str,
    pub legacy: &'a str,
    pub native: &'a str,
    pub confirmation: &'a str,
}
struct RuntimeChecks {
    systemctl: PathBuf,
    ports: [u16; 2],
}
impl Default for RuntimeChecks {
    fn default() -> Self {
        Self {
            systemctl: "systemctl".into(),
            ports: [3550, 13550],
        }
    }
}
pub async fn import_drained(options: ImportOptions<'_>) -> Result<Value> {
    import_drained_checked(options, &RuntimeChecks::default()).await
}
async fn import_drained_checked(
    options: ImportOptions<'_>,
    checks: &RuntimeChecks,
) -> Result<Value> {
    if options.confirmation != "IMPORT_DRAINED_PRIVATE_AI_AFTER_REVIEW" {
        return Err("Explicit reviewed import confirmation required".into());
    }
    let state = options.state.canonicalize()?;
    let backup_parent = options
        .backup
        .parent()
        .ok_or("Backup parent missing")?
        .canonicalize()?;
    if backup_parent.starts_with(&state) || options.backup.exists() {
        return Err("Backup must be new and outside source state".into());
    }
    let bulk = options.bulk.map(Path::canonicalize).transpose()?;
    if bulk
        .as_ref()
        .is_some_and(|path| backup_parent.starts_with(path))
    {
        return Err("Backup cannot be inside bulk data".into());
    }
    for name in ["uploads", "workspaces"] {
        let link = state.join(name);
        if let Ok(resolved) = link.canonicalize()
            && !resolved.starts_with(&state)
            && bulk.as_ref().is_none_or(|root| !resolved.starts_with(root))
        {
            return Err("External private uploads/workspaces require their explicit bulk root in the full backup".into());
        }
    }
    let _lock = ImportLock::acquire(&backup_parent)?;
    stopped_barrier(
        &unit_state_with_tool(options.legacy, &checks.systemctl).await?,
        &unit_state_with_tool(options.native, &checks.systemctl).await?,
    )?;
    ports_idle(&checks.ports).await?;
    let inspection = inspect(&state.join("queue.sqlite")).await?;
    if inspection.active != 0 {
        return Err(
            "Drain all queued/running/paused jobs before import; no cancellation is performed"
                .into(),
        );
    }
    let before = source_fingerprint(&state)?;
    let mut required = tree_bytes(&state)?;
    if let Some(bulk) = &bulk {
        required = required
            .checked_add(tree_bytes(bulk)?)
            .ok_or("Snapshot size overflow")?;
    }
    ensure_capacity(&backup_parent, required)?;
    private_dir(options.backup)?;
    copy_tree(&state, &options.backup.join("state"))?;
    if let Some(bulk) = &bulk {
        copy_tree(bulk, &options.backup.join("bulk"))?
    }
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(options.database)
        .await?;
    let empty = target_fingerprint(&pool).await?;
    if empty.as_object().is_none_or(|tables| {
        tables.len() != TABLES.len() || tables.values().any(|value| value["count"] != 0)
    }) {
        return Err("All provisioned native core and robotics target tables must be empty".into());
    }
    if source_fingerprint(&state)? != before {
        return Err("Legacy source changed while snapshotting; import refused".into());
    }
    // Schema provisioning is deliberately separate. No CREATE/ALTER/DROP here.
    let store = cbgpu_ai_store::Store::new(
        pool.clone(),
        options.backup.join("state"),
        options
            .backup
            .join("bulk")
            .is_dir()
            .then(|| options.backup.join("bulk")),
        Default::default(),
    )?;
    let core = cbgpu_ai_store::importer::import_read_only(
        &store,
        &options.backup.join("state/queue.sqlite"),
    )
    .await?;
    let robotics =
        cbgpu_robotics::import::import_read_only(&pool, &options.backup.join("state/queue.sqlite"))
            .await?;
    stopped_barrier(
        &unit_state_with_tool(options.legacy, &checks.systemctl).await?,
        &unit_state_with_tool(options.native, &checks.systemctl).await?,
    )?;
    ports_idle(&checks.ports).await?;
    if source_fingerprint(&state)? != before {
        return Err("Legacy source changed during handoff; refuse activation".into());
    }
    let target = target_fingerprint(&pool).await?;
    for (table, count) in &inspection.counts {
        if target[table]["count"].as_i64() != Some(*count) {
            return Err("Imported table count differs from the private snapshot".into());
        }
    }
    pool.close().await;
    let report = json!({"version":1,"phase":"imported-not-activated","source_fingerprint":before,"counts":inspection.counts,"core":core,"robotics":robotics,"target_fingerprint":target,"legacy_unit":options.legacy,"native_unit":options.native,"activation_performed":false});
    private_json(&options.backup.join("handoff.json"), &report)?;
    Ok(report)
}
struct ImportLock(File);
impl ImportLock {
    fn acquire(root: &Path) -> Result<Self> {
        use std::os::{fd::AsRawFd, unix::fs::OpenOptionsExt};
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(root.join(".cbgpu-ai-handoff.lock"))?;
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            return Err("Another AI handoff is active".into());
        }
        Ok(Self(file))
    }
}
impl Drop for ImportLock {
    fn drop(&mut self) {
        use std::os::fd::AsRawFd;
        unsafe {
            libc::flock(self.0.as_raw_fd(), libc::LOCK_UN);
        }
    }
}
/// Rollback is only eligible if neither native data nor legacy source has changed.
/// This does not stop/start a service or overwrite either database.
pub async fn rollback_check(state: &Path, backup: &Path, database: &str) -> Result<Value> {
    rollback_check_checked(state, backup, database, &RuntimeChecks::default()).await
}
async fn rollback_check_checked(
    state: &Path,
    backup: &Path,
    database: &str,
    checks: &RuntimeChecks,
) -> Result<Value> {
    let report: Value = serde_json::from_slice(&fs::read(backup.join("handoff.json"))?)?;
    stopped_barrier(
        &unit_state_with_tool(
            report["legacy_unit"]
                .as_str()
                .ok_or("Missing legacy unit")?,
            &checks.systemctl,
        )
        .await?,
        &unit_state_with_tool(
            report["native_unit"]
                .as_str()
                .ok_or("Missing native unit")?,
            &checks.systemctl,
        )
        .await?,
    )?;
    if source_fingerprint(state)? != report["source_fingerprint"] {
        return Err("Legacy source changed; automatic rollback is unsafe".into());
    }
    ports_idle(&checks.ports).await?;
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect(database)
        .await?;
    let matches = target_fingerprint(&pool).await? == report["target_fingerprint"];
    pool.close().await;
    if !matches {
        return Err(
            "Native data changed after import; reconcile new writes before rollback".into(),
        );
    }
    Ok(
        json!({"rollback_eligible":true,"services_changed":false,"databases_changed":false,"required_next_step":"Reviewed operator may restore the legacy unit routing; never discard native writes"}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn readonly_preflight_counts_queued_paused_and_unknown_states_without_changing_source() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("queue.sqlite");
        let pool = SqlitePoolOptions::new()
            .connect_with(
                SqliteConnectOptions::new()
                    .filename(&path)
                    .create_if_missing(true),
            )
            .await
            .unwrap();
        sqlx::raw_sql("CREATE TABLE jobs(id TEXT,state TEXT);INSERT INTO jobs VALUES('a','queued'),('b','paused'),('c','running'),('d','succeeded'),('e','failed'),('f','cancelled'),('g','future-state');").execute(&pool).await.unwrap();
        pool.close().await;
        let before = file_digest(&path).unwrap();
        let info = inspect(&path).await.unwrap();
        assert_eq!(info.active, 4);
        assert_eq!(info.counts["jobs"], 7);
        assert_eq!(file_digest(&path).unwrap(), before);
        let pool = SqlitePoolOptions::new()
            .connect_with(SqliteConnectOptions::new().filename(&path))
            .await
            .unwrap();
        sqlx::query("CREATE TABLE unexpected_private_data(secret TEXT)")
            .execute(&pool)
            .await
            .unwrap();
        pool.close().await;
        assert!(inspect(&path).await.is_err());
    }
    #[test]
    fn no_writer_barrier_requires_both_stopped_and_legacy_masked() {
        let legacy =
            json!({"ActiveState":"inactive","MainPID":"0","UnitFileState":"masked-runtime"});
        let native = json!({"ActiveState":"inactive","MainPID":"0","UnitFileState":"disabled"});
        assert!(stopped_barrier(&legacy, &native).is_ok());
        for value in [
            json!({"ActiveState":"active","MainPID":"1","UnitFileState":"masked-runtime"}),
            json!({"ActiveState":"inactive","MainPID":"0","UnitFileState":"enabled"}),
            json!({}),
        ] {
            assert!(stopped_barrier(&value, &native).is_err())
        }
        assert!(
            stopped_barrier(&legacy, &json!({"ActiveState":"activating","MainPID":"5"})).is_err()
        );
        assert!(unit_name("../unrelated.service").is_err());
    }
    #[test]
    fn private_full_snapshot_preserves_bytes_and_links_without_link_traversal() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        private_dir(&source).unwrap();
        fs::write(source.join("secret"), "private synthetic data").unwrap();
        symlink(".", source.join("loop")).unwrap();
        let target = temp.path().join("snapshot");
        copy_tree(&source, &target).unwrap();
        assert_eq!(
            file_digest(&source.join("secret")).unwrap(),
            file_digest(&target.join("secret")).unwrap()
        );
        assert_eq!(fs::read_link(target.join("loop")).unwrap(), Path::new("."));
        assert_eq!(
            fs::metadata(target.join("secret"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(&target).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(tree_bytes(&source).unwrap(), 22);
    }
    #[tokio::test]
    async fn apply_without_review_cannot_even_inspect_services_or_create_backup() {
        let temp = tempfile::tempdir().unwrap();
        let backup = temp.path().join("backup");
        assert!(
            import_drained(ImportOptions {
                state: temp.path(),
                bulk: None,
                backup: &backup,
                database: "not-a-real-database",
                legacy: "never",
                native: "never",
                confirmation: ""
            })
            .await
            .is_err()
        );
        assert!(!backup.exists());
    }
    #[tokio::test]
    async fn stray_native_listener_blocks_import_even_when_service_state_is_inactive() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        assert!(ports_idle(&[port]).await.is_err());
        drop(listener);
        assert!(ports_idle(&[port]).await.is_ok());
    }
    #[tokio::test]
    #[ignore = "Requires disposable CBGPU_AI_TEST_DATABASE_URL ending /cbgpu_ai_handoff_test; no live units"]
    async fn postgres_full_handoff_core_robotics_backup_and_rollback_contract() {
        let database = std::env::var("CBGPU_AI_TEST_DATABASE_URL").unwrap();
        assert!(database.ends_with("/cbgpu_ai_handoff_test"));
        let temp = tempfile::tempdir().unwrap();
        let state = temp.path().join("state");
        private_dir(&state).unwrap();
        let bulk = temp.path().join("bulk");
        private_dir(&bulk).unwrap();
        private_dir(&bulk.join("uploads")).unwrap();
        std::os::unix::fs::symlink(bulk.join("uploads"), state.join("uploads")).unwrap();
        fs::write(bulk.join("uploads/upload"), "private fixture upload").unwrap();
        fs::create_dir_all(state.join("jobs/job")).unwrap();
        fs::write(
            state.join("jobs/job/result.json"),
            r#"{"private":"retained"}"#,
        )
        .unwrap();
        let source = state.join("queue.sqlite");
        let sqlite = SqlitePoolOptions::new()
            .connect_with(
                SqliteConnectOptions::new()
                    .filename(&source)
                    .create_if_missing(true),
            )
            .await
            .unwrap();
        sqlx::raw_sql(include_str!("fixture.sql"))
            .execute(&sqlite)
            .await
            .unwrap();
        sqlite.close().await;
        let source_bytes = fs::read(&source).unwrap();
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(3)
            .connect(&database)
            .await
            .unwrap();
        let store = cbgpu_ai_store::Store::new(
            pool.clone(),
            temp.path().join("bootstrap"),
            None,
            Default::default(),
        )
        .unwrap();
        store.migrate().await.unwrap();
        cbgpu_robotics::migrate(&pool).await.unwrap();
        let fixture_source = temp.path().join("systemctl.rs");
        fs::write(&fixture_source,r#"fn main(){let args:Vec<_>=std::env::args().collect();assert_eq!(&args[1..3],["--user","show"]);println!("ActiveState=inactive\nSubState=dead\nMainPID=0\nUnitFileState={}",if args[3]=="fixture-legacy.service"{"masked-runtime"}else{"disabled"});}"#).unwrap();
        let tool = temp.path().join("fixture-systemctl");
        assert!(
            std::process::Command::new(std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into()))
                .arg(&fixture_source)
                .arg("-o")
                .arg(&tool)
                .status()
                .unwrap()
                .success()
        );
        let first = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let second = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let checks = RuntimeChecks {
            systemctl: tool,
            ports: [
                first.local_addr().unwrap().port(),
                second.local_addr().unwrap().port(),
            ],
        };
        drop(second);
        let backup = temp.path().join("backup");
        let options = || ImportOptions {
            state: &state,
            bulk: Some(&bulk),
            backup: &backup,
            database: &database,
            legacy: "fixture-legacy.service",
            native: "fixture-native.service",
            confirmation: "IMPORT_DRAINED_PRIVATE_AI_AFTER_REVIEW",
        };
        assert!(
            import_drained_checked(options(), &checks).await.is_err(),
            "stray listener denies import"
        );
        assert!(!backup.exists());
        drop(first);
        let sqlite = SqlitePoolOptions::new()
            .connect_with(SqliteConnectOptions::new().filename(&source))
            .await
            .unwrap();
        sqlx::query("UPDATE jobs SET state='queued'")
            .execute(&sqlite)
            .await
            .unwrap();
        sqlite.close().await;
        assert!(
            import_drained_checked(options(), &checks).await.is_err(),
            "queued job cannot be cancelled by handoff"
        );
        assert!(!backup.exists());
        let sqlite = SqlitePoolOptions::new()
            .connect_with(SqliteConnectOptions::new().filename(&source))
            .await
            .unwrap();
        sqlx::query("UPDATE jobs SET state='succeeded'")
            .execute(&sqlite)
            .await
            .unwrap();
        sqlite.close().await;
        assert_ne!(
            fs::read(&source).unwrap(),
            source_bytes,
            "fixture state updates changed its SQLite snapshot generation"
        );
        let source_bytes = fs::read(&source).unwrap();
        let report = import_drained_checked(options(), &checks).await.unwrap();
        assert_eq!(report["phase"], "imported-not-activated");
        assert_eq!(report["counts"].as_object().unwrap().len(), 12);
        for table in TABLES {
            assert_eq!(
                report["target_fingerprint"][table]["count"],
                if *table == "robot_sync_events" { 0 } else { 1 },
                "{table}"
            );
        }
        assert_eq!(
            fs::read(&source).unwrap(),
            source_bytes,
            "original SQLite bytes must stay untouched"
        );
        assert_eq!(
            fs::read(backup.join("state/queue.sqlite")).unwrap(),
            source_bytes
        );
        assert_eq!(
            fs::read(backup.join("bulk/uploads/upload")).unwrap(),
            b"private fixture upload"
        );
        let restored = temp.path().join("restored");
        copy_tree(&backup.join("state"), &restored).unwrap();
        assert_eq!(
            fs::read(restored.join("queue.sqlite")).unwrap(),
            source_bytes
        );
        assert_eq!(
            fs::read(restored.join("jobs/job/result.json")).unwrap(),
            br#"{"private":"retained"}"#
        );
        let restored_bulk = temp.path().join("restored-bulk");
        copy_tree(&backup.join("bulk"), &restored_bulk).unwrap();
        assert_eq!(
            fs::read(restored_bulk.join("uploads/upload")).unwrap(),
            b"private fixture upload"
        );
        assert_eq!(
            json!(
                inspect(&restored.join("queue.sqlite"))
                    .await
                    .unwrap()
                    .counts
            ),
            report["counts"]
        );
        let before = target_fingerprint(&pool).await.unwrap();
        let repeat = temp.path().join("repeat");
        let mut retry = options();
        retry.backup = &repeat;
        assert!(import_drained_checked(retry, &checks).await.is_err());
        assert_eq!(target_fingerprint(&pool).await.unwrap(), before);
        assert_eq!(
            rollback_check_checked(&state, &backup, &database, &checks)
                .await
                .unwrap()["rollback_eligible"],
            true
        );
        sqlx::query("INSERT INTO ai_store.ai_projects(id,owner,title,created)VALUES('native-new','owner','new native data',5)").execute(&pool).await.unwrap();
        assert!(
            rollback_check_checked(&state, &backup, &database, &checks)
                .await
                .is_err(),
            "native new writes forbid automatic rollback"
        );
        assert_eq!(fs::read(&source).unwrap(), source_bytes);
        pool.close().await;
    }
}
