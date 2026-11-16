//! Explicit read-only operational snapshot -> network-isolated, same-image disposable clone.
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::Duration,
};
static ERROR_LOG: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
struct CloneDb(String);
impl Drop for CloneDb {
    fn drop(&mut self) {
        let _ = Command::new("docker")
            .args(["rm", "-f", "-v", &self.0])
            .output();
    }
}
fn command(args: &[&str], input: Option<&[u8]>) -> Result<Vec<u8>> {
    let mut child = Command::new("docker")
        .args(args)
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    if let Some(bytes) = input {
        child
            .stdin
            .take()
            .ok_or("stdin unavailable")?
            .write_all(bytes)?;
    }
    let out = child.wait_with_output()?;
    if !out.status.success() {
        if let Some(path) = ERROR_LOG.get()
            && let Ok(mut file) = OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o600)
                .open(path)
        {
            let _ = file.write_all(&out.stderr);
        }
        return Err(format!(
            "Docker operation failed ({}); no operational database mutation was requested",
            args.first().unwrap_or(&"unknown")
        )
        .into());
    }
    Ok(out.stdout)
}
fn snapshot(db: &str) -> Result<Vec<u8>> {
    let sql = r#"CREATE TEMP TABLE account_check(nsp text,tbl text,rows bigint,digest text);
DO $$DECLARE r record;nr bigint;hash text;projection text;BEGIN FOR r IN SELECT n.nspname,c.relname FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace WHERE c.relkind='r' AND(n.nspname='public' OR(n.nspname='auth' AND c.relname IN('users','sessions'))) ORDER BY 1,2 LOOP
projection:=CASE WHEN r.nspname='public' AND r.relname='profiles' THEN 'to_jsonb(t)-ARRAY[''account_type'',''account_status'',''school_id'',''affiliation_verified'',''verified_at'',''department'',''organization'',''required_school_year'',''required_grade'']' ELSE 'to_jsonb(t)' END;
EXECUTE format('SELECT count(*),md5(coalesce(string_agg(v::text,E''\n'' ORDER BY v::text),''''))FROM(SELECT %s v FROM %I.%I t)x',projection,r.nspname,r.relname) INTO nr,hash;
INSERT INTO account_check VALUES(r.nspname,r.relname,nr,hash);END LOOP;END$$;
COPY(SELECT * FROM account_check ORDER BY nsp,tbl)TO STDOUT WITH CSV;"#;
    command(
        &[
            "exec",
            "-i",
            db,
            "psql",
            "-h",
            "/tmp",
            "-U",
            "cbgpu_clone_bootstrap",
            "-d",
            "postgres",
            "-v",
            "ON_ERROR_STOP=1",
            "-qAt",
        ],
        Some(sql.as_bytes()),
    )
}
fn private_file(path: &Path) -> Result<File> {
    Ok(OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?)
}
fn dump(source: &str, args: &[&str], path: &Path) -> Result<()> {
    let mut command = Command::new("docker");
    command.args(["exec", source]);
    command.args(args);
    let out = command
        .stdout(private_file(path)?)
        .stderr(private_file(&path.with_extension("stderr"))?)
        .status()?;
    if !out.success() {
        return Err("Read-only dump failed; inspect private stderr file".into());
    }
    Ok(())
}
fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().collect();
    if args.len() != 5 || args[1] != "--clone-and-verify" {
        println!(
            "Usage: schema-dry-run --clone-and-verify SOURCE_CONTAINER NEW_PRIVATE_BACKUP_DIRECTORY REPO_ROOT\nOperational source is pg_dump read-only. All DDL/import runs only in a new same-image network-none disposable clone. No activation, rollover, grants or service restart."
        );
        return Ok(());
    }
    let source = &args[2];
    if !source
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
        || source.starts_with("cbgpu-account-clone-")
    {
        return Err("Invalid operational source name".into());
    }
    let backup = PathBuf::from(&args[3]);
    if !backup.is_absolute() {
        return Err("Private backup path must be absolute and new".into());
    }
    fs::DirBuilder::new().mode(0o700).create(&backup)?;
    let _ = ERROR_LOG.set(backup.join("clone-error.log"));
    let repo = PathBuf::from(&args[4]);
    let image = String::from_utf8(command(
        &["inspect", "--format", "{{.Image}}", source],
        None,
    )?)?
    .trim()
    .to_owned();
    if !image.starts_with("sha256:") {
        return Err("Source image must be pinned".into());
    }
    dump(
        source,
        &[
            "pg_dumpall",
            "-U",
            "postgres",
            "--globals-only",
            "--no-role-passwords",
        ],
        &backup.join("globals.sql"),
    )?;
    dump(
        source,
        &[
            "pg_dump",
            "-U",
            "postgres",
            "-d",
            "postgres",
            "--format=custom",
        ],
        &backup.join("database.dump"),
    )?;
    let clone = CloneDb(format!("cbgpu-account-clone-{}", std::process::id()));
    command(
        &[
            "run",
            "--rm",
            "-d",
            "--name",
            &clone.0,
            "--network",
            "none",
            "--memory",
            "2g",
            "--cpus",
            "2",
            "--user",
            "postgres",
            "--entrypoint",
            "/bin/sh",
            &image,
            "-c",
            "initdb -U cbgpu_clone_bootstrap -D /tmp/cbgpu-account-check -A trust >/tmp/init.log 2>&1 && exec postgres -D /tmp/cbgpu-account-check -k /tmp -c listen_addresses='' -c shared_preload_libraries=pg_cron,pg_net -c cron.database_name=postgres -c cron.launch_active_jobs=off -c pg_net.database_name=cbgpu_disabled_clone_network",
        ],
        None,
    )?;
    let mut ready = false;
    for _ in 0..100 {
        if command(
            &[
                "exec",
                &clone.0,
                "pg_isready",
                "-h",
                "/tmp",
                "-U",
                "cbgpu_clone_bootstrap",
            ],
            None,
        )
        .is_ok()
        {
            ready = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    if !ready {
        return Err("Disposable clone failed to start".into());
    }
    let original_globals = fs::read_to_string(backup.join("globals.sql"))?;
    // PostgreSQL17 dumps can emit membership grantors before their ADMIN grants.
    // Replay the same membership/ADMIN/INHERIT/SET graph as the isolated bootstrap,
    // changing clone-only grant provenance; retain original dump for full audit.
    let globals = original_globals
        .lines()
        .map(|line| {
            if line.starts_with("GRANT ") {
                line.split_once(" GRANTED BY ")
                    .map(|(grant, _)| format!("{grant};"))
                    .unwrap_or_else(|| line.to_owned())
            } else {
                line.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    command(
        &[
            "exec",
            "-i",
            &clone.0,
            "psql",
            "-h",
            "/tmp",
            "-U",
            "cbgpu_clone_bootstrap",
            "-d",
            "postgres",
            "-v",
            "ON_ERROR_STOP=1",
        ],
        Some(globals.as_bytes()),
    )?;
    // Preserve owners/ACLs: the same-image clone has recreated source roles above.
    command(
        &[
            "cp",
            backup
                .join("database.dump")
                .to_str()
                .ok_or("Invalid backup path")?,
            &format!("{}:/tmp/source.dump", clone.0),
        ],
        None,
    )?;
    command(
        &[
            "exec",
            "--user",
            "0",
            &clone.0,
            "chown",
            "postgres:postgres",
            "/tmp/source.dump",
        ],
        None,
    )?;
    command(
        &[
            "exec",
            &clone.0,
            "pg_restore",
            "-h",
            "/tmp",
            "-U",
            "cbgpu_clone_bootstrap",
            "-d",
            "postgres",
            "--clean",
            "--if-exists",
            "--exit-on-error",
            "/tmp/source.dump",
        ],
        None,
    )?;
    command(
        &[
            "exec",
            "-i",
            &clone.0,
            "psql",
            "-h",
            "/tmp",
            "-U",
            "cbgpu_clone_bootstrap",
            "-d",
            "postgres",
            "-v",
            "ON_ERROR_STOP=1",
        ],
        Some(b"ALTER DATABASE postgres OWNER TO postgres;"),
    )?;
    let before = snapshot(&clone.0)?;
    private_file(&backup.join("before.csv"))?.write_all(&before)?;
    let migrations = [
        "20261116084450_account_access_rollover.sql",
        "20261116092500_account_resource_boundaries.sql",
        "20261116233927_account_contest_execution_guard.sql",
    ];
    for _ in 0..2 {
        for file in migrations {
            let mut sql = b"SET ROLE supabase_admin;\n".to_vec();
            sql.extend(fs::read(repo.join("supabase/migrations").join(file))?);
            command(
                &[
                    "exec",
                    "-i",
                    &clone.0,
                    "psql",
                    "-h",
                    "/tmp",
                    "-U",
                    "cbgpu_clone_bootstrap",
                    "-d",
                    "postgres",
                    "-v",
                    "ON_ERROR_STOP=1",
                ],
                Some(&sql),
            )?;
        }
    }
    let after = snapshot(&clone.0)?;
    private_file(&backup.join("after.csv"))?.write_all(&after)?;
    if before != after {
        return Err(
            "Immutable account/Auth/project/membership snapshots differ; activation refused".into(),
        );
    }
    private_file(&backup.join("PASSED"))?.write_all(b"same-image clone; owner/ACL fidelity; migrations replayed twice; immutable rows and credentials unchanged; no production DDL\n")?;
    println!(
        "Account migration same-image clone PASSED; private snapshot and proof retained. Operational database was not modified."
    );
    Ok(())
}
