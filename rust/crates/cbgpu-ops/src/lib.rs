//! Native operational replacement for the former Python provisioning helpers.
//! No command runs unless explicitly selected; no application data is reset.
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::{io::AsyncWriteExt, process::Command};
pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
pub const UNITS: [&str; 3] = [
    "cbgpu-rust-core.service",
    "cbgpu-study-auto.service",
    "cbgpu-timetable-refresh.service",
];
#[derive(Clone)]
pub struct Context {
    pub root: PathBuf,
    pub home: PathBuf,
    pub container: String,
    pub tool_dir: Option<PathBuf>,
    pub readiness_url: String,
}
#[derive(Serialize, Deserialize, Debug)]
pub struct Manifest {
    pub previous_release: Option<PathBuf>,
    pub units: BTreeMap<String, bool>,
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
fn random() -> String {
    let mut bytes = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    hex(&bytes)
}
pub fn digest(path: &Path) -> Result<String> {
    Ok(hex(&Sha256::digest(fs::read(path)?)))
}
pub fn parse_env(text: &str) -> BTreeMap<String, String> {
    text.lines()
        .filter_map(|line| {
            if line.trim_start().starts_with('#') {
                return None;
            }
            let (k, v) = line.split_once('=')?;
            Some((k.trim().to_owned(), v.to_owned()))
        })
        .collect()
}
fn private_dir(path: &Path) -> Result<()> {
    fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}
pub fn atomic_write(path: &Path, bytes: &[u8], mode: u32) -> Result<()> {
    let parent = path.parent().ok_or("Missing output parent")?;
    let temp = parent.join(format!(
        ".cbgpu-ops-{}-{}",
        std::process::id(),
        &random()[..12]
    ));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(mode);
    }
    let mut file = options.open(&temp)?;
    if let Err(e) = file.write_all(bytes).and_then(|_| file.sync_all()) {
        let _ = fs::remove_file(&temp);
        return Err(e.into());
    }
    if let Err(e) = fs::rename(&temp, path) {
        let _ = fs::remove_file(&temp);
        return Err(e.into());
    }
    Ok(())
}
fn remove_if_exists(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}
#[cfg(unix)]
fn link_release(base: &Path, target: Option<&Path>) -> Result<()> {
    let current = base.join("current");
    if let Some(target) = target {
        let temporary = base.join(format!(".candidate-{}", &random()[..12]));
        std::os::unix::fs::symlink(target, &temporary)?;
        if let Err(e) = fs::rename(&temporary, &current) {
            let _ = fs::remove_file(temporary);
            return Err(e.into());
        }
    } else {
        remove_if_exists(&current)?;
    }
    Ok(())
}
#[cfg(not(unix))]
fn link_release(_: &Path, _: Option<&Path>) -> Result<()> {
    Err("Operational systemd installation requires Unix".into())
}
async fn command(
    program: &Path,
    args: &[String],
    input: Option<&str>,
    env: &BTreeMap<String, String>,
    cwd: Option<&Path>,
    seconds: u64,
) -> Result<String> {
    let mut cmd = Command::new(program);
    cmd.args(args)
        .envs(env)
        .kill_on_drop(true)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        });
    if let Some(cwd) = cwd {
        cmd.current_dir(cwd);
    }
    let mut child = cmd
        .spawn()
        .map_err(|_| "Could not start required operational command")?;
    if let Some(input) = input {
        child
            .stdin
            .take()
            .ok_or("Missing command stdin")?
            .write_all(input.as_bytes())
            .await?;
    }
    let output = tokio::time::timeout(Duration::from_secs(seconds), child.wait_with_output())
        .await
        .map_err(|_| "Operational command timed out; credentials were not logged")??;
    if !output.status.success() {
        return Err("Operational command failed; credentials were not logged".into());
    }
    Ok(String::from_utf8(output.stdout)?.trim().to_owned())
}
impl Context {
    fn tool(&self, name: &str) -> PathBuf {
        self.tool_dir
            .as_ref()
            .map(|p| p.join(name))
            .unwrap_or_else(|| PathBuf::from(name))
    }
    fn base(&self) -> PathBuf {
        self.home.join(".local/lib/cbgpu-rust")
    }
    fn units(&self) -> PathBuf {
        self.home.join(".config/systemd/user")
    }
    fn config(&self) -> PathBuf {
        self.home.join(".config/cbgpu")
    }
    async fn sql(&self, query: &str, database: &str) -> Result<String> {
        command(
            &self.tool("docker"),
            &[
                "exec",
                "-i",
                &self.container,
                "psql",
                "-X",
                "-A",
                "-t",
                "-q",
                "-v",
                "ON_ERROR_STOP=1",
                "-U",
                "postgres",
                "-d",
                database,
            ]
            .map(str::to_owned),
            Some(query),
            &BTreeMap::new(),
            None,
            20,
        )
        .await
    }
    async fn system(&self, args: &[&str]) -> Result<()> {
        let args = std::iter::once("--user")
            .chain(args.iter().copied())
            .map(str::to_owned)
            .collect::<Vec<_>>();
        command(
            &self.tool("systemctl"),
            &args,
            None,
            &BTreeMap::new(),
            None,
            30,
        )
        .await?;
        Ok(())
    }
    pub async fn provision(&self) -> Result<serde_json::Value> {
        private_dir(&self.config())?;
        let env_file = self.config().join("rust-core.env");
        if env_file.exists() {
            return Err("Runtime configuration exists; refusing to replace credentials".into());
        }
        let roles=self.sql("SELECT count(*) FROM pg_roles WHERE rolname IN ('cbgpu_runtime','cbgpu_runtime_owner');","postgres").await?;
        let databases = self
            .sql(
                "SELECT count(*) FROM pg_database WHERE datname='cbgpu_runtime';",
                "postgres",
            )
            .await?;
        if roles != "0" || databases != "0" {
            return Err("Runtime roles/database exist; refusing overwrite".into());
        }
        let password = random();
        let owner_password = random();
        // Preserve recovery credentials before the first database mutation. Never log them.
        let recovery = self.config().join("rust-core-provision-recovery.json");
        if recovery.exists() {
            return Err("Existing provisioning recovery record requires manual review".into());
        }
        atomic_write(
            &recovery,
            &serde_json::to_vec(
                &serde_json::json!({"runtime_password":password,"owner_password":owner_password}),
            )?,
            0o600,
        )?;
        self.sql(&format!("CREATE ROLE cbgpu_runtime LOGIN PASSWORD '{password}' NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION NOINHERIT CONNECTION LIMIT 8; CREATE ROLE cbgpu_runtime_owner LOGIN PASSWORD '{owner_password}' NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION NOINHERIT CONNECTION LIMIT 2;"),"postgres").await?;
        self.sql(
            "GRANT cbgpu_runtime_owner TO postgres WITH SET TRUE;",
            "postgres",
        )
        .await?;
        let created = self
            .sql(
                "CREATE DATABASE cbgpu_runtime OWNER cbgpu_runtime_owner;",
                "postgres",
            )
            .await;
        let revoked = self
            .sql("REVOKE cbgpu_runtime_owner FROM postgres;", "postgres")
            .await;
        created?;
        revoked?;
        self.sql("REVOKE ALL ON DATABASE cbgpu_runtime FROM PUBLIC; GRANT CONNECT ON DATABASE cbgpu_runtime TO cbgpu_runtime; REVOKE CREATE ON SCHEMA public FROM PUBLIC;","cbgpu_runtime").await?;
        let ip = command(
            &self.tool("docker"),
            &[
                "inspect",
                "--format",
                "{{range .NetworkSettings.Networks}}{{.IPAddress}}{{end}}",
                &self.container,
            ]
            .map(str::to_owned),
            None,
            &BTreeMap::new(),
            None,
            20,
        )
        .await?;
        let _: std::net::IpAddr = ip
            .parse()
            .map_err(|_| "Database container has no unique private IP")?;
        let make_url = |user: &str, password: &str| -> Result<String> {
            let mut u = url::Url::parse(&format!("postgresql://{user}@{ip}:5432/cbgpu_runtime"))?;
            u.set_password(Some(password))
                .map_err(|_| "Could not construct private DSN")?;
            Ok(u.to_string())
        };
        let runtime_url = make_url("cbgpu_runtime", &password)?;
        let owner_url = make_url("cbgpu_runtime_owner", &owner_password)?;
        let env = BTreeMap::from([("CBGPU_RUNTIME_DATABASE_URL".to_owned(), owner_url)]);
        command(
            &self.root.join("target/release/cbgpu-core"),
            &["migrate".to_owned()],
            None,
            &env,
            None,
            60,
        )
        .await?;
        self.sql("ALTER ROLE cbgpu_runtime_owner NOLOGIN;", "postgres")
            .await?;
        let key = self.config().join("rust-core-admin-key");
        if !key.exists() {
            atomic_write(&key, format!("{}\n", random()).as_bytes(), 0o600)?;
        }
        atomic_write(&env_file,format!("CBGPU_RUNTIME_DATABASE_URL={runtime_url}\nCBGPU_RUST_LISTEN=127.0.0.1:3540\nCBGPU_WEB_ORIGIN=http://127.0.0.1:3000\nCBGPU_RUST_ADMIN_KEY_FILE={}\n",key.display()).as_bytes(),0o600)?;
        remove_if_exists(&recovery)?;
        Ok(
            serde_json::json!({"ok":true,"database":"cbgpu_runtime","runtime_superuser":false,"runtime_owns_tables":false,"existing_application_tables_changed":false}),
        )
    }
    pub fn load_manifest(&self, backup: &Path) -> Result<(PathBuf, Manifest)> {
        let root = self
            .home
            .join(".local/share/cbgpu-rust/rollbacks")
            .canonicalize()?;
        let backup = backup.canonicalize()?;
        if backup.parent() != Some(root.as_path()) {
            return Err("Rollback must be a direct child of private manifest directory".into());
        }
        let manifest: Manifest = serde_json::from_slice(&fs::read(backup.join("manifest.json"))?)?;
        let keys = manifest
            .units
            .keys()
            .map(String::as_str)
            .collect::<std::collections::BTreeSet<_>>();
        if keys != UNITS.into_iter().collect() {
            return Err("Invalid rollback unit set".into());
        }
        if let Some(previous) = &manifest.previous_release {
            let path = previous.canonicalize()?;
            if path.parent() != Some(self.base().join("releases").canonicalize()?.as_path())
                || !path.is_dir()
            {
                return Err("Prior release escapes private release directory".into());
            }
        }
        for (name, exists) in &manifest.units {
            if *exists && !fs::symlink_metadata(backup.join(name))?.is_file() {
                return Err("Rollback unit is not a regular file".into());
            }
        }
        Ok((backup, manifest))
    }
    fn restore_files(&self, backup: &Path, manifest: &Manifest) -> Result<()> {
        for (name, exists) in &manifest.units {
            let path = self.units().join(name);
            if *exists {
                atomic_write(&path, &fs::read(backup.join(name))?, 0o600)?;
            } else {
                remove_if_exists(&path)?;
            }
        }
        link_release(&self.base(), manifest.previous_release.as_deref())
    }
    pub async fn rollback(&self, backup: &Path) -> Result<serde_json::Value> {
        let (backup, manifest) = self.load_manifest(backup)?;
        self.restore_files(&backup, &manifest)?;
        self.system(&["daemon-reload"]).await?;
        self.system(&[
            if manifest.previous_release.is_some() {
                "restart"
            } else {
                "stop"
            },
            "cbgpu-rust-core.service",
        ])
        .await?;
        Ok(
            serde_json::json!({"ok":true,"timers_preserved":true,"user_services_restarted":false,"database_rolled_back":false}),
        )
    }
    pub async fn install(&self) -> Result<serde_json::Value> {
        let binary = self.root.join("target/release/cbgpu-core");
        if !binary.is_file() {
            return Err("Build release cbgpu-core before installation".into());
        }
        let env = parse_env(&fs::read_to_string(self.config().join("rust-core.env"))?);
        for operation in ["study-auto", "timetable-refresh"] {
            command(
                &binary,
                &[operation.to_owned(), "--dry-run".to_owned()],
                None,
                &env,
                None,
                15,
            )
            .await?;
        }
        let hash = digest(&binary)?;
        private_dir(&self.base())?;
        let release = self.base().join("releases").join(&hash[..20]);
        private_dir(&release)?;
        let target = release.join("cbgpu-core");
        if !target.exists() {
            atomic_write(&target, &fs::read(&binary)?, 0o755)?;
        }
        if digest(&target)? != hash {
            return Err("Installed binary checksum mismatch".into());
        }
        private_dir(&self.units())?;
        let backup = self
            .home
            .join(".local/share/cbgpu-rust/rollbacks")
            .join(format!(
                "{}-{}",
                chrono::Utc::now().format("%Y%m%dT%H%M%S"),
                &random()[..8]
            ));
        private_dir(&backup)?;
        let current = self.base().join("current");
        let previous = if current.exists() {
            Some(current.canonicalize()?)
        } else {
            None
        };
        let mut manifest = Manifest {
            previous_release: previous,
            units: BTreeMap::new(),
        };
        for name in UNITS {
            let path = self.units().join(name);
            let exists = path.exists();
            manifest.units.insert(name.to_owned(), exists);
            if exists {
                atomic_write(&backup.join(name), &fs::read(path)?, 0o600)?;
            }
        }
        atomic_write(
            &backup.join("manifest.json"),
            &serde_json::to_vec(&manifest)?,
            0o600,
        )?;
        // Validate all rollback paths before modifying units or current link.
        self.load_manifest(&backup)?;
        let commit = command(
            &self.tool("git"),
            &["rev-parse", "HEAD"].map(str::to_owned),
            None,
            &BTreeMap::new(),
            Some(&self.root),
            15,
        )
        .await?;
        if commit.len() != 40 || !commit.bytes().all(|c| c.is_ascii_hexdigit()) {
            return Err("Invalid source commit".into());
        }
        let mut units = Vec::new();
        for name in UNITS {
            let unit = fs::read_to_string(self.root.join("deploy").join(name))?;
            if !unit.contains("[Service]") {
                return Err("Invalid source unit".into());
            }
            units.push((
                name,
                unit.replacen(
                    "[Service]",
                    &format!("[Service]\nEnvironment=CBGPU_RELEASE_SHA={commit}"),
                    1,
                ),
            ));
        }
        link_release(&self.base(), Some(&release))?;
        let installed: Result<()> = async {
            for (name, unit) in units {
                atomic_write(&self.units().join(name), unit.as_bytes(), 0o600)?;
            }
            self.system(&["daemon-reload"]).await?;
            self.system(&["enable", "cbgpu-rust-core.service"]).await?;
            self.system(&["restart", "cbgpu-rust-core.service"]).await?;
            let client = reqwest::Client::builder()
                .timeout(Duration::from_secs(2))
                .build()?;
            for _ in 0..30 {
                if let Ok(response) = client.get(&self.readiness_url).send().await
                    && response.status().is_success()
                    && let Ok(body) = response.json::<serde_json::Value>().await
                    && body["status"] == "ok"
                {
                    return Ok(());
                }
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
            Err("Rust readiness failed".into())
        }
        .await;
        if let Err(error) = installed {
            self.restore_files(&backup, &manifest)?;
            self.system(&["daemon-reload"]).await?;
            self.system(&[
                if manifest.previous_release.is_some() {
                    "restart"
                } else {
                    "stop"
                },
                "cbgpu-rust-core.service",
            ])
            .await?;
            return Err(error);
        }
        Ok(
            serde_json::json!({"ok":true,"binary_sha256":hash,"rollback":backup,"user_services_restarted":false,"existing_timers_preserved":true}),
        )
    }
    pub async fn integration(&self) -> Result<serde_json::Value> {
        let env = parse_env(&fs::read_to_string(self.config().join("rust-core.env"))?);
        let mut url = url::Url::parse(
            env.get("CBGPU_RUNTIME_DATABASE_URL")
                .ok_or("Runtime database URL missing")?,
        )?;
        url.set_path("/cbgpu_runtime_test");
        // Collision refusal is intentional: never DROP an existing unrelated database.
        self.sql(
            "GRANT cbgpu_runtime_owner TO postgres WITH SET TRUE;",
            "postgres",
        )
        .await?;
        let created = self
            .sql(
                "CREATE DATABASE cbgpu_runtime_test OWNER cbgpu_runtime_owner;",
                "postgres",
            )
            .await;
        if created.is_err() {
            self.sql("REVOKE cbgpu_runtime_owner FROM postgres;", "postgres")
                .await?;
            created?;
        }
        let result:Result<()>=async{let files=fs::read_dir(self.root.join("migrations"))?.filter_map(|e|e.ok()).filter(|e|e.file_name().to_string_lossy().ends_with("_rust_operations_core.sql")).collect::<Vec<_>>();if files.len()!=1{return Err("Expected exactly one core migration".into());}let migration=fs::read_to_string(files[0].path())?;self.sql(&format!("REVOKE ALL ON DATABASE cbgpu_runtime_test FROM PUBLIC;GRANT CONNECT ON DATABASE cbgpu_runtime_test TO cbgpu_runtime;SET ROLE cbgpu_runtime_owner;{migration}"),"cbgpu_runtime_test").await?;
 let vars=BTreeMap::from([("CBGPU_TEST_DATABASE_URL".to_owned(),url.to_string()),("CARGO_BUILD_JOBS".to_owned(),"4".to_owned())]);command(&self.home.join(".cargo/bin/cargo"),&["test".to_owned(),"--locked".to_owned(),"--manifest-path".to_owned(),self.root.join("Cargo.toml").to_string_lossy().to_string(),"-p".to_owned(),"cbgpu-core".to_owned(),"--test".to_owned(),"postgres".to_owned(),"--".to_owned(),"--ignored".to_owned()],None,&vars,None,300).await?;Ok(())}.await;
        let drop_result = self
            .sql("DROP DATABASE cbgpu_runtime_test;", "postgres")
            .await;
        let revoke_result = self
            .sql("REVOKE cbgpu_runtime_owner FROM postgres;", "postgres")
            .await;
        drop_result?;
        revoke_result?;
        result?;
        Ok(
            serde_json::json!({"ok":true,"isolated_database":"cbgpu_runtime_test","existing_application_tables_changed":false}),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn context(home: &Path) -> Context {
        Context {
            root: home.join("source"),
            home: home.to_owned(),
            container: "synthetic-never-used".into(),
            tool_dir: None,
            readiness_url: "http://127.0.0.1:1".into(),
        }
    }
    #[test]
    fn env_preserves_equals_and_ignores_comments() {
        let parsed = parse_env("# ignore=x\nA=secret=with=equals\nB=value\ninvalid\n");
        assert_eq!(parsed["A"], "secret=with=equals");
        assert_eq!(parsed.len(), 2);
    }
    #[test]
    fn atomic_private_write_and_hash() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("private");
        atomic_write(&p, b"abc", 0o600).unwrap();
        assert_eq!(
            digest(&p).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(fs::metadata(p).unwrap().permissions().mode() & 0o777, 0o600);
        }
    }
    #[test]
    fn rollback_rejects_external_directory_before_any_write() {
        let t = tempfile::tempdir().unwrap();
        let c = context(t.path());
        private_dir(&c.home.join(".local/share/cbgpu-rust/rollbacks")).unwrap();
        assert!(c.load_manifest(t.path()).is_err());
        assert!(!c.units().exists());
    }
    #[test]
    fn invalid_manifest_cannot_restore_unapproved_unit() {
        let t = tempfile::tempdir().unwrap();
        let c = context(t.path());
        let b = c.home.join(".local/share/cbgpu-rust/rollbacks/test");
        private_dir(&b).unwrap();
        atomic_write(
            &b.join("manifest.json"),
            br#"{"previous_release":null,"units":{"ssh.service":true}}"#,
            0o600,
        )
        .unwrap();
        assert!(c.load_manifest(&b).is_err());
        assert!(!c.units().exists());
    }
    #[test]
    fn rollback_restores_only_saved_units_and_release() {
        let t = tempfile::tempdir().unwrap();
        let c = context(t.path());
        let release = c.base().join("releases/old");
        private_dir(&release).unwrap();
        private_dir(&c.units()).unwrap();
        let b = c.home.join(".local/share/cbgpu-rust/rollbacks/test");
        private_dir(&b).unwrap();
        let mut m = Manifest {
            previous_release: Some(release.clone()),
            units: BTreeMap::new(),
        };
        for name in UNITS {
            m.units.insert(name.to_owned(), name == UNITS[0]);
            atomic_write(&c.units().join(name), b"candidate", 0o600).unwrap();
        }
        atomic_write(&b.join(UNITS[0]), b"previous", 0o600).unwrap();
        atomic_write(
            &b.join("manifest.json"),
            &serde_json::to_vec(&m).unwrap(),
            0o600,
        )
        .unwrap();
        let (b, m) = c.load_manifest(&b).unwrap();
        c.restore_files(&b, &m).unwrap();
        assert_eq!(fs::read(c.units().join(UNITS[0])).unwrap(), b"previous");
        assert!(!c.units().join(UNITS[1]).exists());
        assert_eq!(
            c.base().join("current").canonicalize().unwrap(),
            release.canonicalize().unwrap()
        );
    }
    #[cfg(unix)]
    #[test]
    fn rollback_rejects_prior_release_symlink_escape() {
        let t = tempfile::tempdir().unwrap();
        let c = context(t.path());
        let releases = c.base().join("releases");
        private_dir(&releases).unwrap();
        std::os::unix::fs::symlink(t.path(), releases.join("escape")).unwrap();
        let b = c.home.join(".local/share/cbgpu-rust/rollbacks/test");
        private_dir(&b).unwrap();
        let m = Manifest {
            previous_release: Some(releases.join("escape")),
            units: UNITS.into_iter().map(|s| (s.to_owned(), false)).collect(),
        };
        atomic_write(
            &b.join("manifest.json"),
            &serde_json::to_vec(&m).unwrap(),
            0o600,
        )
        .unwrap();
        assert!(c.load_manifest(&b).is_err());
    }
    #[tokio::test]
    async fn provision_refuses_existing_configuration_before_running_docker() {
        let t = tempfile::tempdir().unwrap();
        let c = context(t.path());
        private_dir(&c.config()).unwrap();
        atomic_write(&c.config().join("rust-core.env"), b"existing", 0o600).unwrap();
        let e = c.provision().await.unwrap_err().to_string();
        assert!(e.contains("refusing"));
        assert_eq!(
            fs::read(c.config().join("rust-core.env")).unwrap(),
            b"existing"
        );
    }
}
