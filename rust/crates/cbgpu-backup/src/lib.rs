use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::process::Command;
pub type Result<T> = std::result::Result<T, &'static str>;
pub struct Config {
    pub runtime: PathBuf,
    pub helper_root: PathBuf,
    pub helper_extension: String,
}
#[derive(Debug)]
pub struct Export {
    pub filename: &'static str,
    pub args: Vec<&'static str>,
}
pub fn exports() -> Vec<Export> {
    vec![
        Export {
            filename: "database.dump",
            args: vec![
                "exec",
                "cbgpu-supabase-db",
                "pg_dump",
                "-U",
                "supabase_admin",
                "--format=custom",
                "postgres",
            ],
        },
        Export {
            filename: "runtime-database.dump",
            args: vec![
                "exec",
                "cbgpu-supabase-db",
                "pg_dump",
                "-U",
                "supabase_admin",
                "--format=custom",
                "cbgpu_runtime",
            ],
        },
        Export {
            filename: "snapshot-counts.json",
            args: vec![
                "exec",
                "cbgpu-supabase-db",
                "psql",
                "-X",
                "-U",
                "supabase_admin",
                "-d",
                "postgres",
                "-At",
                "-c",
                "SELECT json_build_object('users',(SELECT count(*) FROM auth.users),'profiles',(SELECT count(*) FROM public.profiles),'public_tables',(SELECT count(*) FROM information_schema.tables WHERE table_schema='public'),'files',(SELECT count(*) FROM storage.objects));",
            ],
        },
        Export {
            filename: "roles.sql",
            args: vec![
                "exec",
                "cbgpu-supabase-db",
                "pg_dumpall",
                "-U",
                "supabase_admin",
                "--roles-only",
            ],
        },
        Export {
            filename: "storage.tar.gz",
            args: vec![
                "exec",
                "cbgpu-supabase-storage",
                "tar",
                "-czf",
                "-",
                "-C",
                "/var/lib/storage",
                ".",
            ],
        },
        Export {
            filename: "database-encryption-keys.tar.gz",
            args: vec![
                "exec",
                "cbgpu-supabase-db",
                "tar",
                "-czf",
                "-",
                "-C",
                "/etc/postgresql-custom",
                ".",
            ],
        },
    ]
}
fn private_file(path: &Path) -> Result<File> {
    let mut options = OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
        .open(path)
        .map_err(|_| "Cannot create private backup file")
}
async fn run(
    program: &str,
    args: &[&str],
    stdout: Option<File>,
    stdin: Option<File>,
    runtime: &Path,
) -> Result<()> {
    let mut command = Command::new(program);
    command
        .args(args)
        .env("CBGPU_RUNTIME_BACKEND", runtime)
        .stdout(stdout.map(Stdio::from).unwrap_or_else(Stdio::null))
        .stdin(stdin.map(Stdio::from).unwrap_or_else(Stdio::null))
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let mut child = command.spawn().map_err(|_| "Cannot start backup tool")?;
    match tokio::time::timeout(Duration::from_secs(900), child.wait()).await {
        Ok(Ok(status)) if status.success() => Ok(()),
        _ => {
            let _ = child.kill().await;
            Err("Backup step failed; partial snapshot was retained privately")
        }
    }
}
pub fn checksums(root: &Path, names: &[&str]) -> Result<String> {
    let mut text = String::new();
    let mut buffer = [0_u8; 65536];
    for name in names {
        let mut file = File::open(root.join(name)).map_err(|_| "Missing backup component")?;
        let mut hash = Sha256::new();
        loop {
            let count = file
                .read(&mut buffer)
                .map_err(|_| "Cannot hash backup component")?;
            if count == 0 {
                break;
            }
            hash.update(&buffer[..count]);
        }
        text.push_str(&format!("{:x}  {name}\n", hash.finalize()));
    }
    Ok(text)
}
pub async fn backup(config: &Config) -> Result<PathBuf> {
    if !config.runtime.is_absolute()
        || !config.helper_root.is_absolute()
        || !["mts", "mjs"].contains(&config.helper_extension.as_str())
    {
        return Err("Use absolute private runtime and trusted helper paths");
    }
    let runtime = config
        .runtime
        .canonicalize()
        .map_err(|_| "Runtime backend is unavailable")?;
    let helper = config.helper_root.join(format!(
        "scripts/backup-developer.{}",
        config.helper_extension
    ));
    let offsite = runtime.join(format!("operations/backup.{}", config.helper_extension));
    if !helper.is_file() || !offsite.is_file() {
        return Err("Trusted metadata/offsite compatibility helpers are unavailable");
    }
    for name in [".env", "functions.env"] {
        if !runtime.join(name).is_file() {
            return Err("Runtime configuration is incomplete");
        }
    }
    let parent = runtime.join("backups");
    fs::create_dir_all(&parent).map_err(|_| "Cannot create backup parent")?;
    let folder = parent.join(chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string());
    fs::create_dir(&folder).map_err(|_| "Snapshot already exists or cannot be created")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&folder, fs::Permissions::from_mode(0o700))
            .map_err(|_| "Cannot protect backup directory")?;
    }
    private_file(&folder.join(".incomplete"))?;
    for export in exports() {
        run(
            "docker",
            &export.args,
            Some(private_file(&folder.join(export.filename))?),
            None,
            &runtime,
        )
        .await?;
    }
    run(
        "tar",
        &[
            "-czf",
            folder
                .join("materials.tar.gz")
                .to_str()
                .ok_or("Invalid backup path")?,
            "--exclude=*.partial",
            "--exclude=*.upload.json",
            "--exclude=.cbgpu-materials.lock",
            "-C",
            "/home/cbgpu/cbgpu-materials",
            ".",
        ],
        None,
        None,
        &runtime,
    )
    .await?;
    for name in [".env", "functions.env"] {
        let mut target = private_file(&folder.join(name))?;
        let mut source = File::open(runtime.join(name))
            .map_err(|_| "Cannot open private runtime configuration")?;
        std::io::copy(&mut source, &mut target)
            .map_err(|_| "Cannot snapshot runtime configuration")?;
        target
            .sync_all()
            .map_err(|_| "Cannot persist runtime snapshot")?;
    }
    run(
        "node",
        &[
            helper.to_str().ok_or("Invalid helper path")?,
            folder.to_str().ok_or("Invalid backup path")?,
        ],
        None,
        None,
        &runtime,
    )
    .await?;
    // pg_restore --list verifies both archives before any offsite upload; it does not restore a database.
    for name in ["database.dump", "runtime-database.dump"] {
        run(
            "docker",
            &["exec", "-i", "cbgpu-supabase-db", "pg_restore", "--list"],
            None,
            Some(File::open(folder.join(name)).map_err(|_| "Missing database archive")?),
            &runtime,
        )
        .await?;
    }
    let value = checksums(
        &folder,
        &[
            "database.dump",
            "runtime-database.dump",
            "roles.sql",
            "storage.tar.gz",
            "materials.tar.gz",
            "database-encryption-keys.tar.gz",
        ],
    )?;
    let mut file = private_file(&folder.join("checksums.sha256"))?;
    file.write_all(value.as_bytes())
        .map_err(|_| "Cannot persist backup checksums")?;
    file.sync_all()
        .map_err(|_| "Cannot persist backup checksums")?;
    fs::remove_file(folder.join(".incomplete")).map_err(|_| "Cannot mark snapshot complete")?;
    run(
        "node",
        &[offsite.to_str().ok_or("Invalid offsite helper")?, "upload"],
        None,
        None,
        &runtime,
    )
    .await?;
    Ok(folder)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn includes_application_runtime_database_and_cluster_roles() {
        let exports = exports();
        assert!(
            exports.iter().any(|e| e.filename == "runtime-database.dump"
                && e.args.last() == Some(&"cbgpu_runtime"))
        );
        assert!(
            exports
                .iter()
                .any(|e| e.filename == "database.dump" && e.args.last() == Some(&"postgres"))
        );
        assert!(
            exports
                .iter()
                .any(|e| e.filename == "roles.sql" && e.args.contains(&"--roles-only"))
        );
    }
    #[test]
    fn checksums_cover_snapshot_bytes() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("sample"), b"abc").unwrap();
        assert_eq!(
            checksums(dir.path(), &["sample"]).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad  sample\n"
        );
        assert!(checksums(dir.path(), &["missing"]).is_err());
    }
    #[tokio::test]
    async fn invalid_runtime_fails_before_creating_files() {
        let dir = tempfile::tempdir().unwrap();
        let config = Config {
            runtime: dir.path().into(),
            helper_root: dir.path().into(),
            helper_extension: "mts".into(),
        };
        assert!(backup(&config).await.is_err());
        assert!(!dir.path().join("backups").exists());
    }
}
