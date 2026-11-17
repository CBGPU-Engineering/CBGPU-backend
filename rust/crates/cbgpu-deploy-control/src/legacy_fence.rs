//! Explicit operator admission fence for the existing Node supervisor SQLite.
//! No worker/service/container is started or stopped and no project data is changed.
use crate::{Result, crypto::Cipher, fail};
use futures_util::TryStreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{Connection, Row, SqliteConnection, sqlite::SqliteConnectOptions};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    time::Duration,
};
use uuid::Uuid;
const FORMAT: &str = "cbgpu-deploy-legacy-fence-v1";
#[derive(Clone, Serialize, Deserialize, Debug, PartialEq, Eq)]
struct Trigger {
    name: String,
    sql: String,
    sha256: String,
}
#[derive(Clone, Serialize, Deserialize)]
struct Manifest {
    format: String,
    id: String,
    database: PathBuf,
    device: u64,
    inode: u64,
    key_sha256: String,
    triggers: Vec<Trigger>,
}
fn hash(bytes: impl AsRef<[u8]>) -> String {
    format!("{:x}", Sha256::digest(bytes.as_ref()))
}
#[cfg(unix)]
fn existing(path: &Path, key: bool) -> Result<(PathBuf, File, u64, u64)> {
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
    if !path.is_absolute() {
        return Err(fail("Existing absolute database/key paths required", 400));
    }
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    let meta = file.metadata()?;
    if !meta.is_file()
        || meta.uid() != unsafe { libc::getuid() }
        || key && (meta.len() != 32 || meta.permissions().mode() & 0o077 != 0)
    {
        return Err(fail(
            "Existing private operator-owned database/key required",
            403,
        ));
    }
    let canonical = path.canonicalize()?;
    let current = fs::metadata(&canonical)?;
    if current.dev() != meta.dev() || current.ino() != meta.ino() {
        return Err(fail("Private state identity changed", 409));
    }
    Ok((canonical, file, meta.dev(), meta.ino()))
}
#[cfg(not(unix))]
fn existing(_path: &Path, _key: bool) -> Result<(PathBuf, File, u64, u64)> {
    Err(fail(
        "SQLite handoff requires reviewed Unix filesystem identity",
        503,
    ))
}
struct Identity {
    database: PathBuf,
    device: u64,
    inode: u64,
    key_sha256: String,
    cipher: Cipher,
    _database_file: File,
}
impl Identity {
    fn load(database: &Path, key: &Path) -> Result<Self> {
        let (database, mut file, device, inode) = existing(database, false)?;
        let mut header = [0; 16];
        file.read_exact(&mut header)?;
        if &header != b"SQLite format 3\0" {
            return Err(fail("Existing SQLite database required", 400));
        }
        let (_, mut key_file, _, _) = existing(key, true)?;
        let mut bytes = [0; 32];
        key_file.read_exact(&mut bytes)?;
        Ok(Self {
            database,
            device,
            inode,
            key_sha256: hash(bytes),
            cipher: Cipher::new(&bytes)?,
            _database_file: file,
        })
    }
    async fn connect(&self, readonly: bool) -> Result<SqliteConnection> {
        let c = SqliteConnection::connect_with(
            &SqliteConnectOptions::new()
                .filename(&self.database)
                .create_if_missing(false)
                .read_only(readonly)
                .busy_timeout(Duration::from_secs(5)),
        )
        .await?;
        let (_, _, dev, ino) = existing(&self.database, false)?;
        if dev != self.device || ino != self.inode {
            return Err(fail("SQLite state changed during handoff", 409));
        }
        Ok(c)
    }
    fn matches(&self, m: &Manifest) -> bool {
        m.format == FORMAT
            && m.database == self.database
            && m.device == self.device
            && m.inode == self.inode
            && m.key_sha256 == self.key_sha256
    }
}
fn triggers(id: &str) -> Result<Vec<Trigger>> {
    let uuid = Uuid::parse_str(id).map_err(|_| fail("Invalid fence receipt identity", 400))?;
    let prefix = format!("cbgpu_handoff_{}", uuid.simple());
    Ok([("deployments_insert","BEFORE INSERT ON deployments"),("compose_insert","BEFORE INSERT ON compose_states"),("compose_update","BEFORE UPDATE ON compose_states WHEN NEW.status IN ('queued','building')")].into_iter().map(|(name,event)|{let name=format!("{prefix}_{name}");let sql=format!("CREATE TRIGGER {name} {event} BEGIN SELECT RAISE(ABORT,'CBGPU deployment handoff fence active'); END");Trigger{sha256:hash(&sql),name,sql}}).collect())
}
async fn counts(c: &mut SqliteConnection) -> Result<(i64, i64)> {
    let front: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM deployments WHERE status IN ('queued','building')",
    )
    .fetch_one(&mut *c)
    .await?;
    let compose: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM compose_states WHERE status IN ('queued','building')",
    )
    .fetch_one(&mut *c)
    .await?;
    Ok((front, compose))
}
async fn verify_key(c: &mut SqliteConnection, cipher: &Cipher) -> Result<()> {
    let mut rows = sqlx::query("SELECT secrets FROM projects LIMIT 100001").fetch(&mut *c);
    let mut count = 0;
    while let Some(row) = rows.try_next().await? {
        count += 1;
        if count > 100000 {
            return Err(fail("Project-key verification bound exceeded", 409));
        }
        let value: String = row.try_get("secrets")?;
        if value.len() > 2 * 1024 * 1024 {
            return Err(fail("Project ciphertext verification bound exceeded", 409));
        }
        cipher.unseal(&value)?;
    }
    Ok(())
}
struct NewReceipt {
    path: PathBuf,
    file: File,
    keep: bool,
}
impl NewReceipt {
    fn reserve(path: &Path) -> Result<Self> {
        if !path.is_absolute() {
            return Err(fail("Absolute new private receipt path required", 400));
        }
        let parent = path
            .parent()
            .ok_or_else(|| fail("Receipt parent required", 400))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::{MetadataExt, PermissionsExt};
            let m = fs::metadata(parent)?;
            if !m.is_dir()
                || m.uid() != unsafe { libc::getuid() }
                || m.permissions().mode() & 0o077 != 0
            {
                return Err(fail(
                    "Receipt parent must be private and operator-owned",
                    403,
                ));
            }
        }
        let mut options = OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        }
        let file = options.open(path)?;
        Ok(Self {
            path: path.to_owned(),
            file,
            keep: false,
        })
    }
    fn persist(&mut self, value: &Value) -> Result<()> {
        let bytes = serde_json::to_vec(value)?;
        self.file.write_all(&bytes)?;
        self.file.sync_all()?;
        File::open(self.path.parent().unwrap())?.sync_all()?;
        self.keep = true;
        Ok(())
    }
}
impl Drop for NewReceipt {
    fn drop(&mut self) {
        if !self.keep {
            let _ = fs::remove_file(&self.path);
        }
    }
}
fn read_receipt(path: &Path, identity: &Identity) -> Result<Manifest> {
    let (_, mut f, _, _) = existing(path, false)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if f.metadata()?.permissions().mode() & 0o077 != 0 {
            return Err(fail("Fence receipt must remain private", 403));
        }
    }
    if f.metadata()?.len() > 65536 {
        return Err(fail("Fence receipt exceeds bound", 400));
    }
    let mut bytes = Vec::new();
    f.read_to_end(&mut bytes)?;
    let envelope: Value = serde_json::from_slice(&bytes)?;
    if envelope["format"] != FORMAT {
        return Err(fail("Unknown fence receipt format", 400));
    }
    let value = identity.cipher.unseal(
        envelope["sealed_receipt"]
            .as_str()
            .ok_or_else(|| fail("Authenticated fence receipt required", 400))?,
    )?;
    let m: Manifest = serde_json::from_value(value)?;
    if !identity.matches(&m) || m.triggers != triggers(&m.id)? {
        return Err(fail(
            "Fence receipt does not match this database/key/trigger contract",
            409,
        ));
    }
    Ok(m)
}
/// Default read-only plan. Point-in-time counts are not themselves an admission fence.
pub async fn plan(database: &Path, key: &Path) -> Result<Value> {
    let identity = Identity::load(database, key)?;
    let mut c = identity.connect(true).await?;
    let mut tx = c.begin().await?;
    let (front, compose) = counts(&mut tx).await?;
    verify_key(&mut tx, &identity.cipher).await?;
    tx.rollback().await?;
    Ok(
        json!({"action":"plan","database":identity.database,"active_deployments":front,"active_compose":compose,"can_fence":front==0&&compose==0,"changed":false,"external_workers_inspected":false}),
    )
}
/// Counts and admission triggers share one reserved-write transaction; busy refusal changes nothing.
pub async fn apply(database: &Path, key: &Path, receipt: &Path) -> Result<Value> {
    let identity = Identity::load(database, key)?;
    let mut c = identity.connect(false).await?;
    let mut tx = c.begin_with("BEGIN IMMEDIATE").await?;
    let (front, compose) = counts(&mut tx).await?;
    if front != 0 || compose != 0 {
        return Err(fail(
            "Deployment/Compose work is active; refusing handoff fence",
            409,
        ));
    }
    verify_key(&mut tx, &identity.cipher).await?;
    let present: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM sqlite_master WHERE type='trigger' AND name LIKE 'cbgpu_handoff_%'",
    )
    .fetch_one(&mut *tx)
    .await?;
    if present != 0 {
        return Err(fail(
            "An existing handoff fence must be reviewed first",
            409,
        ));
    }
    let id = Uuid::new_v4().to_string();
    let owned = triggers(&id)?;
    let mut output = NewReceipt::reserve(receipt)?;
    for trigger in &owned {
        sqlx::query(sqlx::AssertSqlSafe(trigger.sql.as_str()))
            .execute(&mut *tx)
            .await?;
        let actual: String =
            sqlx::query_scalar("SELECT sql FROM sqlite_master WHERE type='trigger' AND name=?")
                .bind(&trigger.name)
                .fetch_one(&mut *tx)
                .await?;
        if actual != trigger.sql {
            return Err(fail("SQLite trigger contract differs", 409));
        }
    }
    let m = Manifest {
        format: FORMAT.into(),
        id,
        database: identity.database,
        device: identity.device,
        inode: identity.inode,
        key_sha256: identity.key_sha256,
        triggers: owned,
    };
    let sealed = identity.cipher.seal(&serde_json::to_value(m)?)?;
    output.persist(&json!({"format":FORMAT,"sealed_receipt":sealed}))?;
    // The receipt is durable first. An uncertain COMMIT retains it for inspection rather than losing the only unfence proof.
    tx.commit().await?;
    Ok(
        json!({"state":"fenced","receipt":receipt,"owned_triggers":3,"active_deployments":0,"active_compose":0,"external_workers_inspected":false}),
    )
}
/// Validates every own trigger before dropping any; other actors' triggers are never touched.
pub async fn remove(database: &Path, key: &Path, receipt: &Path) -> Result<Value> {
    let identity = Identity::load(database, key)?;
    let m = read_receipt(receipt, &identity)?;
    let mut c = identity.connect(false).await?;
    let mut tx = c.begin_with("BEGIN IMMEDIATE").await?;
    let mut found = 0;
    for trigger in &m.triggers {
        let actual: Option<String> =
            sqlx::query_scalar("SELECT sql FROM sqlite_master WHERE type='trigger' AND name=?")
                .bind(&trigger.name)
                .fetch_optional(&mut *tx)
                .await?;
        if let Some(actual) = actual {
            if actual != trigger.sql || hash(&actual) != trigger.sha256 {
                return Err(fail("Fence trigger changed; refusing any removal", 409));
            }
            found += 1
        }
    }
    if found != 0 && found != m.triggers.len() {
        return Err(fail(
            "Fence trigger set is incomplete; refusing any removal",
            409,
        ));
    }
    for trigger in &m.triggers {
        if found > 0 {
            sqlx::query(sqlx::AssertSqlSafe(format!(
                "DROP TRIGGER {}",
                trigger.name
            )))
            .execute(&mut *tx)
            .await?;
        }
    }
    tx.commit().await?;
    Ok(
        json!({"state":if found==0{"already_absent"}else{"unfenced"},"owned_triggers_removed":found,"receipt_preserved":true,"other_triggers_modified":false}),
    )
}
