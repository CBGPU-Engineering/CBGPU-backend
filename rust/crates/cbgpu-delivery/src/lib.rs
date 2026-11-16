//! Native delivery policy and transactional filesystem/systemd core.
//! GitHub CI verification is an authenticated API check, not a commit signature.
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io::{Read, Write},
    os::unix::fs::OpenOptionsExt,
    path::{Component, Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};
pub type Result<T> = std::result::Result<T, String>;

pub fn valid_sha(sha: &str) -> bool {
    sha.len() == 40
        && sha
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
pub fn allowed_repo(repo: &str) -> bool {
    matches!(
        repo,
        "CBGPU-Engineering/CBGPU-web" | "CBGPU-Engineering/CBGPU-backend"
    )
}
pub fn safe_relative(path: &str) -> bool {
    !path.is_empty()
        && !path.contains('\\')
        && !path.contains('\0')
        && path
            .split('/')
            .all(|s| !s.is_empty() && s != "." && s != "..")
        && Path::new(path)
            .components()
            .all(|c| matches!(c, Component::Normal(_)))
}
pub fn unit_name(name: &str) -> bool {
    name.starts_with("cbgpu-")
        && name.ends_with(".service")
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
}
pub fn protected(path: &str) -> bool {
    [
        "schema/",
        "supabase/",
        "volumes/",
        "gpu/",
        "latex/",
        "rust/migrations/",
    ]
    .iter()
    .any(|p| path.starts_with(p))
        || [
            "docker-compose.yml",
            ".gitmodules",
            "package.json",
            "package-lock.json",
        ]
        .contains(&path)
}
pub fn changed_paths(compare: &Value) -> Result<Vec<String>> {
    if !matches!(compare["status"].as_str(), Some("ahead" | "identical")) {
        return Err("Main history diverged; manual review required".into());
    }
    let files = compare["files"]
        .as_array()
        .ok_or("Missing GitHub comparison files")?;
    if files.len() >= 300 {
        return Err("Change set too large for unattended rollout".into());
    }
    files
        .iter()
        .map(|f| {
            let p = f["filename"].as_str().ok_or("Missing changed path")?;
            if !safe_relative(p) {
                return Err("Unsafe changed path".into());
            }
            Ok(p.to_owned())
        })
        .collect()
}
pub fn ci_gate(repo: &str, reference: &Value, runs: &Value) -> Result<(String, String)> {
    if !allowed_repo(repo) {
        return Err("Repository is not allowed".into());
    }
    let sha = reference["object"]["sha"]
        .as_str()
        .ok_or("Invalid main reference")?;
    if !valid_sha(sha) {
        return Err("Invalid main commit".into());
    }
    let run = runs["workflow_runs"]
        .as_array()
        .and_then(|items| {
            items.iter().find(|r| {
                r["head_sha"] == sha
                    && r["head_branch"] == "main"
                    && r["event"] == "push"
                    && r["status"] == "completed"
                    && r["conclusion"] == "success"
                    && r["repository"]["full_name"] == repo
            })
        })
        .ok_or("Waiting for successful main-branch push CI")?;
    let url = run["html_url"].as_str().ok_or("Missing CI evidence URL")?;
    if !url.starts_with(&format!("https://github.com/{repo}/actions/runs/")) {
        return Err("Unexpected CI evidence URL".into());
    }
    Ok((sha.to_owned(), url.to_owned()))
}

pub fn atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().ok_or("Missing parent")?;
    fs::create_dir_all(parent).map_err(|_| "Cannot create private directory")?;
    if fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink()) {
        return Err("Refusing symlink output".into());
    }
    let mut seq = 0;
    let (temp, mut f) = loop {
        let temp = parent.join(format!(".delivery-tmp-{}-{seq}", std::process::id()));
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temp)
        {
            Ok(f) => break (temp, f),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => seq += 1,
            Err(_) => return Err("Cannot create private atomic output".into()),
        }
    };
    let result = (|| {
        f.write_all(bytes)
            .map_err(|_| "Cannot write private output")?;
        f.sync_all().map_err(|_| "Cannot sync private output")?;
        fs::rename(&temp, path).map_err(|_| "Cannot commit private output")?;
        fs::File::open(parent)
            .and_then(|d| d.sync_all())
            .map_err(|_| "Cannot sync private directory")?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(temp);
    }
    result
}
pub fn read_json(path: &Path) -> Result<Value> {
    match fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|_| "Invalid private JSON".into()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Value::Null),
        Err(_) => Err("Cannot read private JSON".into()),
    }
}
pub fn visible_state(mut state: Value) -> Value {
    if let Some(components) = state.get_mut("components").and_then(Value::as_object_mut) {
        for value in components.values_mut() {
            if let Some(fields) = value.as_object_mut() {
                fields.remove("rollback_units");
            }
        }
    }
    state
}
pub fn lock(root: &Path) -> Result<fs::File> {
    fs::create_dir_all(root).map_err(|_| "Cannot create delivery home")?;
    let f = OpenOptions::new()
        .append(true)
        .create(true)
        .mode(0o600)
        .open(root.join("agent.lock"))
        .map_err(|_| "Cannot open delivery lock")?;
    f.try_lock_exclusive()
        .map_err(|_| "Another delivery operation is running")?;
    Ok(f)
}

/// Execute argv without a shell; output is never included in public errors.
pub fn command(args: &[String], cwd: Option<&Path>, timeout: Duration) -> Result<String> {
    let first = args.first().ok_or("Empty command")?;
    let mut cmd = Command::new(first);
    cmd.args(&args[1..]);
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    // File capture avoids pipe backpressure and unbounded retained output.
    let capture_directory =
        tempfile::tempdir().map_err(|_| "Cannot create private command capture")?;
    let root = capture_directory.path();
    let capture = root.join("output");
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&capture)
        .map_err(|_| "Concurrent command capture")?;
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::from(
            file.try_clone().map_err(|_| "Cannot clone capture")?,
        ))
        .stderr(Stdio::from(file))
        .spawn()
        .map_err(|_| "Command could not start")?;
    let deadline = Instant::now() + timeout;
    let result = loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                if status.success() {
                    let mut bytes = Vec::new();
                    let result = fs::File::open(&capture)
                        .and_then(|f| f.take(16 * 1024 * 1024).read_to_end(&mut bytes));
                    break result
                        .map(|_| String::from_utf8_lossy(&bytes).trim().to_owned())
                        .map_err(|_| "Cannot read command result".into());
                }
                break Err("Command failed".into());
            }
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break Err("Command timed out".into());
            }
        }
    };
    let _ = fs::remove_file(capture);
    let _ = fs::remove_dir(root);
    result
}
pub fn github(route: &str) -> Result<Value> {
    let output = command(
        &[
            "gh".into(),
            "api".into(),
            route.into(),
            "--method".into(),
            "GET".into(),
        ],
        None,
        Duration::from_secs(45),
    )?;
    serde_json::from_str(&output).map_err(|_| "Invalid GitHub API response".into())
}
pub fn candidate(repo: &str) -> Result<(String, String)> {
    if !allowed_repo(repo) {
        return Err("Repository is not allowed".into());
    }
    let reference = github(&format!("repos/{repo}/git/ref/heads/main"))?;
    let sha = reference["object"]["sha"]
        .as_str()
        .filter(|sha| valid_sha(sha))
        .ok_or("Invalid main reference")?;
    let runs = github(&format!(
        "repos/{repo}/actions/workflows/ci.yml/runs?branch=main&event=push&head_sha={sha}&per_page=5"
    ))?;
    ci_gate(repo, &reference, &runs)
}

pub fn unit_update(
    original: &str,
    directory: &Path,
    exec: &str,
    environment: &BTreeMap<String, String>,
) -> Result<String> {
    let dir = directory.to_str().ok_or("Invalid release path")?;
    if dir.contains(['\n', '\r', '%'])
        || exec.contains(['\n', '\r', '%'])
        || !directory.is_absolute()
    {
        return Err("Unsafe systemd directive".into());
    }
    for (k, v) in environment {
        if k.is_empty()
            || !k
                .bytes()
                .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
            || v.contains(['\n', '\r', '%', '"'])
        {
            return Err("Unsafe systemd environment".into());
        }
    }
    let (mut wd, mut start, mut service) = (0, 0, 0);
    let mut lines = Vec::new();
    for line in original.lines() {
        if line == "[Service]" {
            service += 1;
            lines.push(line.to_owned());
            for (k, v) in environment {
                lines.push(format!("Environment={k}={v}"));
            }
        } else if line.starts_with("WorkingDirectory=") {
            wd += 1;
            lines.push(format!("WorkingDirectory={dir}"))
        } else if line.starts_with("ExecStart=") {
            start += 1;
            lines.push(format!("ExecStart={exec}"))
        } else if !environment
            .keys()
            .any(|k| line.starts_with(&format!("Environment={k}=")))
        {
            lines.push(line.to_owned())
        }
    }
    if (wd, start, service) != (1, 1, 1) {
        return Err("Unexpected systemd unit structure".into());
    }
    Ok(lines.join("\n") + "\n")
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Check {
    pub url: String,
    #[serde(default)]
    pub commit: Option<String>,
    #[serde(default)]
    pub host: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Journal {
    pub units: BTreeMap<String, String>,
    pub checks: Vec<Check>,
}
pub trait Host {
    fn reload(&mut self) -> Result<()>;
    fn restart(&mut self, name: &str) -> Result<()>;
    fn health(&mut self, check: &Check, recovery: bool) -> Result<()>;
}
fn restore(settings: &Path, journal: &Journal, host: &mut impl Host) -> Result<()> {
    for (name, text) in &journal.units {
        if !unit_name(name) {
            return Err("Invalid rollback unit".into());
        }
        atomic(&settings.join(name), text.as_bytes())?;
    }
    host.reload()?;
    for name in journal.units.keys() {
        host.restart(name)?;
    }
    for check in &journal.checks {
        host.health(check, true)?;
    }
    Ok(())
}
pub fn recover(root: &Path, settings: &Path, host: &mut impl Host) -> Result<bool> {
    let raw = read_json(&root.join("transaction.json"))?;
    if raw.is_null() {
        return Ok(false);
    }
    let journal: Journal =
        serde_json::from_value(raw).map_err(|_| "Invalid transaction journal")?;
    restore(settings, &journal, host)?;
    fs::remove_file(root.join("transaction.json"))
        .map_err(|_| "Cannot remove recovered journal")?;
    Ok(true)
}
pub fn switch(
    root: &Path,
    settings: &Path,
    units: &BTreeMap<String, String>,
    checks: Vec<Check>,
    host: &mut impl Host,
) -> Result<BTreeMap<String, String>> {
    let mut before = BTreeMap::new();
    for name in units.keys() {
        if !unit_name(name) {
            return Err("Invalid service name".into());
        }
        let text = fs::read_to_string(settings.join(name))
            .map_err(|_| "Cannot read previous service unit")?;
        before.insert(name.clone(), text);
    }
    let journal = Journal {
        units: before.clone(),
        checks,
    };
    atomic(
        &root.join("transaction.json"),
        &serde_json::to_vec(&journal).map_err(|_| "Cannot serialize journal")?,
    )?;
    let attempt = (|| {
        for (name, text) in units {
            atomic(&settings.join(name), text.as_bytes())?;
        }
        host.reload()?;
        for name in units.keys() {
            host.restart(name)?;
        }
        for check in &journal.checks {
            host.health(check, false)?;
        }
        Ok(())
    })();
    if let Err(error) = attempt {
        restore(settings, &journal, host).map_err(|_| {
            "Release failed AND rollback health failed; journal retained".to_owned()
        })?;
        fs::remove_file(root.join("transaction.json"))
            .map_err(|_| "Cannot remove rolled-back journal")?;
        return Err(error);
    }
    fs::remove_file(root.join("transaction.json")).map_err(|_| "Cannot finish transaction")?;
    Ok(before)
}

fn regular_beneath(root: &Path, relative: &str) -> Option<PathBuf> {
    if !safe_relative(relative) {
        return None;
    }
    let mut path = root.to_owned();
    for part in Path::new(relative).components() {
        path.push(part);
        if fs::symlink_metadata(&path).ok()?.file_type().is_symlink() {
            return None;
        }
    }
    if !path.is_file()
        || !path
            .canonicalize()
            .ok()?
            .starts_with(root.canonicalize().ok()?)
    {
        return None;
    }
    Some(path)
}
/// Retain only public chunks; never copy private server files or follow symlinks.
pub fn retain_web_assets(directory: &Path, previous: &[PathBuf]) -> Result<usize> {
    let target = directory.join(".next-production/static");
    let index = directory.join(".delivery-own-static.json");
    if !index.exists() {
        let own: Vec<_> = walkdir::WalkDir::new(&target)
            .follow_links(false)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_file())
            .filter_map(|e| {
                e.path()
                    .strip_prefix(&target)
                    .ok()
                    .map(|p| p.to_string_lossy().into_owned())
            })
            .collect();
        atomic(
            &index,
            &serde_json::to_vec(&own).map_err(|_| "Cannot encode static index")?,
        )?;
    }
    let mut copied = 0;
    for release in previous.iter().take(3) {
        let source = release.join(".next-production/static");
        let own = read_json(&release.join(".delivery-own-static.json"))?;
        let files: Vec<String> = if own.is_null() {
            walkdir::WalkDir::new(&source)
                .follow_links(false)
                .into_iter()
                .filter_map(|e| e.ok())
                .filter(|e| e.file_type().is_file())
                .filter_map(|e| {
                    e.path()
                        .strip_prefix(&source)
                        .ok()
                        .map(|p| p.to_string_lossy().into_owned())
                })
                .collect()
        } else {
            serde_json::from_value(own).map_err(|_| "Invalid static asset index")?
        };
        for name in files {
            let Some(src) = regular_beneath(&source, &name) else {
                continue;
            };
            let dst = target.join(&name);
            if dst.exists() || fs::symlink_metadata(&dst).is_ok() {
                continue;
            }
            let mut current = target.clone();
            let relative = Path::new(&name);
            let mut safe = true;
            for part in relative.parent().unwrap_or(Path::new("")).components() {
                current.push(part);
                if fs::symlink_metadata(&current).is_ok_and(|m| m.file_type().is_symlink()) {
                    safe = false;
                    break;
                }
            }
            if !safe {
                continue;
            }
            let bytes = fs::read(src).map_err(|_| "Cannot read public chunk")?;
            atomic(&dst, &bytes)?;
            copied += 1;
        }
    }
    Ok(copied)
}
/// Remove secret values before any persisted build log, including failed builds.
pub fn redact(output: &str, environment: &BTreeMap<String, String>) -> String {
    let mut text = output.to_owned();
    let mut secrets: Vec<_> = environment
        .iter()
        .filter(|(k, v)| {
            v.len() > 8
                && [
                    "SECRET",
                    "PASSWORD",
                    "TOKEN",
                    "SERVICE_ROLE",
                    "RESEND",
                    "SMTP_PASS",
                ]
                .iter()
                .any(|pattern| k.contains(pattern))
        })
        .map(|(_, v)| v)
        .collect();
    secrets.sort_by_key(|s| std::cmp::Reverse(s.len()));
    for secret in secrets {
        text = text.replace(secret, "[REDACTED]")
    }
    text
}
pub fn plan_backend(paths: &[String], approved: bool, busy: bool) -> Result<Value> {
    let protected: Vec<_> = paths.iter().filter(|p| protected(p)).collect();
    if !protected.is_empty() && !approved {
        return Err("Infrastructure/schema changes require separate explicit approval".into());
    }
    if busy {
        return Err("User build is active; do not interrupt workers".into());
    }
    Ok(
        json!({"status":"validated_policy","changes":paths.len(),"protected_changes":protected,"apply_supported":true,"reason":"Service data handoffs remain separately approved; rollout never migrates data implicitly"}),
    )
}
pub trait FunctionHost {
    fn restart(&mut self) -> Result<()>;
    fn healthy(&mut self) -> Result<()>;
}
/// Copy only changed function files; preserve backups and restore on failed health.
pub fn sync_functions(
    directory: &Path,
    runtime: &Path,
    paths: &[String],
    sha: &str,
    host: &mut impl FunctionHost,
) -> Result<Vec<String>> {
    if !valid_sha(sha) {
        return Err("Invalid function release commit".into());
    }
    let mut applied: Vec<(String, Option<Vec<u8>>)> = Vec::new();
    let attempt = (|| {
        for name in paths {
            if !name.starts_with("volumes/functions/") {
                continue;
            }
            let Some(source) = regular_beneath(directory, name) else {
                continue;
            };
            let target = runtime.join(name);
            let mut parent = runtime.to_owned();
            for part in Path::new(name)
                .parent()
                .ok_or("Missing function parent")?
                .components()
            {
                parent.push(part);
                if fs::symlink_metadata(&parent).is_ok_and(|m| m.file_type().is_symlink()) {
                    return Err("Unsafe runtime function parent".into());
                }
            }
            if fs::symlink_metadata(&target).is_ok_and(|m| m.file_type().is_symlink()) {
                return Err("Unsafe runtime function target".into());
            }
            let before = match fs::read(&target) {
                Ok(bytes) => Some(bytes),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
                Err(_) => return Err("Cannot snapshot existing function".into()),
            };
            if let Some(bytes) = &before {
                atomic(
                    &runtime
                        .join("backups")
                        .join(format!("functions-{}", &sha[..12]))
                        .join(name),
                    bytes,
                )?
            }
            atomic(
                &target,
                &fs::read(source).map_err(|_| "Cannot read candidate function")?,
            )?;
            applied.push((name.to_owned(), before));
        }
        if !applied.is_empty() {
            host.restart()?;
            host.healthy()?;
        }
        Ok(())
    })();
    if let Err(error) = attempt {
        let mut restoration_failed = false;
        for (name, before) in applied.iter().rev() {
            let result = match before {
                Some(bytes) => atomic(&runtime.join(name), bytes),
                None => fs::remove_file(runtime.join(name))
                    .map_err(|_| "Cannot remove new failed function".into()),
            };
            if result.is_err() {
                restoration_failed = true;
            }
        }
        if !applied.is_empty() && (host.restart().is_err() || host.healthy().is_err()) {
            restoration_failed = true;
        }
        return Err(if restoration_failed {
            "Function update and rollback health failed; backups retained".into()
        } else {
            error
        });
    }
    Ok(applied.into_iter().map(|(name, _)| name).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{PermissionsExt, symlink};
    use tempfile::tempdir;
    const REPO: &str = "CBGPU-Engineering/CBGPU-web";
    fn fixtures() -> (Value, Value) {
        let sha = "a".repeat(40);
        (
            json!({"object":{"sha":sha}}),
            json!({"workflow_runs":[{"head_sha":sha,"head_branch":"main","event":"push","status":"completed","conclusion":"success","repository":{"full_name":REPO},"html_url":format!("https://github.com/{REPO}/actions/runs/1")}]}),
        )
    }
    #[test]
    fn ci_requires_exact_repo_sha_push_success() {
        let (reference, runs) = fixtures();
        assert!(ci_gate(REPO, &reference, &runs).is_ok());
        for (field, value) in [
            ("event", "pull_request"),
            ("head_branch", "feature"),
            ("head_sha", "b"),
            ("conclusion", "failure"),
            ("status", "in_progress"),
        ] {
            let mut bad = runs.clone();
            bad["workflow_runs"][0][field] = json!(value);
            assert!(ci_gate(REPO, &reference, &bad).is_err())
        }
        let mut bad = runs;
        bad["workflow_runs"][0]["repository"]["full_name"] = json!("other/repo");
        assert!(ci_gate(REPO, &reference, &bad).is_err());
        assert!(ci_gate("other/repo", &reference, &bad).is_err())
    }
    #[test]
    fn comparison_and_protected_changes_fail_closed() {
        assert!(changed_paths(&json!({"status":"diverged","files":[]})).is_err());
        assert!(
            changed_paths(&json!({"status":"ahead","files":[{"filename":"../private"}]})).is_err()
        );
        for path in [
            "schema/x.sql",
            "supabase/migrations/x.sql",
            "rust/migrations/x.sql",
            "gpu/server.mts",
            "package.json",
        ] {
            assert!(plan_backend(&[path.into()], false, false).is_err())
        }
        assert!(plan_backend(&["developer/server.mts".into()], false, true).is_err());
        assert!(plan_backend(&["developer/server.mts".into()], false, false).is_ok())
    }
    #[test]
    fn atomic_outputs_private_and_refuse_symlinks() {
        let tmp = tempdir().unwrap();
        let file = tmp.path().join("state.json");
        atomic(&file, b"one").unwrap();
        atomic(&file, b"two").unwrap();
        assert_eq!(fs::read_to_string(&file).unwrap(), "two");
        assert_eq!(
            fs::metadata(&file).unwrap().permissions().mode() & 0o777,
            0o600
        );
        symlink(&file, tmp.path().join("link")).unwrap();
        assert!(atomic(&tmp.path().join("link"), b"bad").is_err());
        assert_eq!(fs::read_to_string(file).unwrap(), "two")
    }
    #[derive(Default)]
    struct MockHost {
        health: usize,
        fail: bool,
        rollback_fail: bool,
        restarts: usize,
    }
    impl Host for MockHost {
        fn reload(&mut self) -> Result<()> {
            Ok(())
        }
        fn restart(&mut self, _: &str) -> Result<()> {
            self.restarts += 1;
            Ok(())
        }
        fn health(&mut self, _: &Check, recovery: bool) -> Result<()> {
            self.health += 1;
            if self.fail && (!recovery || self.rollback_fail) {
                Err("unhealthy".into())
            } else {
                Ok(())
            }
        }
    }
    fn check() -> Check {
        Check {
            url: "http://127.0.0.1:3000/api/health".into(),
            commit: Some("a".repeat(40)),
            host: None,
        }
    }
    #[test]
    fn failed_switch_restores_units_and_requires_rollback_health() {
        let tmp = tempdir().unwrap();
        let settings = tmp.path().join("units");
        fs::create_dir(&settings).unwrap();
        let unit = settings.join("cbgpu-frontend.service");
        fs::write(&unit, "previous").unwrap();
        let units = BTreeMap::from([("cbgpu-frontend.service".into(), "candidate".into())]);
        let mut host = MockHost {
            fail: true,
            ..Default::default()
        };
        assert!(switch(tmp.path(), &settings, &units, vec![check()], &mut host).is_err());
        assert_eq!(fs::read_to_string(&unit).unwrap(), "previous");
        assert_eq!(host.health, 2);
        assert!(!tmp.path().join("transaction.json").exists());
        host.rollback_fail = true;
        assert!(switch(tmp.path(), &settings, &units, vec![check()], &mut host).is_err());
        assert!(tmp.path().join("transaction.json").exists())
    }
    #[test]
    fn interrupted_switch_recovery_replays_recorded_units() {
        let tmp = tempdir().unwrap();
        let settings = tmp.path().join("units");
        fs::create_dir(&settings).unwrap();
        fs::write(settings.join("cbgpu-frontend.service"), "candidate").unwrap();
        let journal = Journal {
            units: BTreeMap::from([("cbgpu-frontend.service".into(), "previous".into())]),
            checks: vec![check()],
        };
        atomic(
            &tmp.path().join("transaction.json"),
            &serde_json::to_vec(&journal).unwrap(),
        )
        .unwrap();
        assert!(recover(tmp.path(), &settings, &mut MockHost::default()).unwrap());
        assert_eq!(
            fs::read_to_string(settings.join("cbgpu-frontend.service")).unwrap(),
            "previous"
        );
        assert!(!recover(tmp.path(), &settings, &mut MockHost::default()).unwrap())
    }
    #[test]
    fn unit_directives_reject_injection_and_ambiguous_units() {
        let unit = "[Service]\nWorkingDirectory=/old\nExecStart=/old/server\n";
        assert!(unit_update(unit, Path::new("/new"), "/new/server", &BTreeMap::new()).is_ok());
        assert!(
            unit_update(
                unit,
                Path::new("/new\nExecStart=evil"),
                "/new/server",
                &BTreeMap::new()
            )
            .is_err()
        );
        assert!(
            unit_update(
                "[Service]\n",
                Path::new("/new"),
                "/new/server",
                &BTreeMap::new()
            )
            .is_err()
        );
        assert!(!unit_name("../../x.service"))
    }
    #[test]
    fn chunks_keep_current_assets_and_exclude_symlinked_private_code() {
        let tmp = tempdir().unwrap();
        let old = tmp.path().join("old");
        let new = tmp.path().join("new");
        for dir in [&old, &new] {
            fs::create_dir_all(dir.join(".next-production/static/chunks")).unwrap()
        }
        fs::write(old.join(".next-production/static/chunks/old.js"), "old").unwrap();
        fs::write(
            old.join(".next-production/static/chunks/shared.js"),
            "previous",
        )
        .unwrap();
        fs::write(
            new.join(".next-production/static/chunks/shared.js"),
            "current",
        )
        .unwrap();
        fs::create_dir_all(old.join(".next-production/server")).unwrap();
        fs::write(old.join(".next-production/server/secret"), "private").unwrap();
        symlink(
            old.join(".next-production/server/secret"),
            old.join(".next-production/static/chunks/link.js"),
        )
        .unwrap();
        assert_eq!(retain_web_assets(&new, &[old]).unwrap(), 1);
        assert_eq!(
            fs::read_to_string(new.join(".next-production/static/chunks/shared.js")).unwrap(),
            "current"
        );
        assert!(!new.join(".next-production/static/chunks/link.js").exists());
        assert!(
            !fs::read_to_string(new.join(".delivery-own-static.json"))
                .unwrap()
                .contains("old.js")
        )
    }
    #[test]
    fn private_logs_redact_longest_overlapping_secrets() {
        let env = BTreeMap::from([
            ("TOKEN".into(), "private-secret".into()),
            ("PASSWORD".into(), "private-secret-extended".into()),
            ("PUBLIC".into(), "visible".into()),
        ]);
        assert_eq!(
            redact("private-secret-extended private-secret visible", &env),
            "[REDACTED] [REDACTED] visible"
        )
    }
    #[test]
    fn lock_prevents_parallel_delivery() {
        let tmp = tempdir().unwrap();
        let _lock = lock(tmp.path()).unwrap();
        assert!(lock(tmp.path()).is_err())
    }
    #[test]
    fn command_timeout_and_failures_never_expose_captured_output() {
        assert!(
            command(
                &[
                    "/bin/sh".into(),
                    "-c".into(),
                    "echo private-secret; exit 1".into()
                ],
                None,
                Duration::from_secs(1)
            )
            .unwrap_err()
            .find("private-secret")
            .is_none()
        );
        assert!(
            command(
                &["/bin/sleep".into(), "2".into()],
                None,
                Duration::from_millis(30)
            )
            .is_err()
        )
    }
    #[derive(Default)]
    struct MockFunctions {
        restarts: usize,
        failed: bool,
    }
    impl FunctionHost for MockFunctions {
        fn restart(&mut self) -> Result<()> {
            self.restarts += 1;
            Ok(())
        }
        fn healthy(&mut self) -> Result<()> {
            if self.failed {
                self.failed = false;
                Err("bad health".into())
            } else {
                Ok(())
            }
        }
    }
    #[test]
    fn function_sync_scoped_atomic_and_reversible() {
        let tmp = tempdir().unwrap();
        let src = tmp.path().join("source");
        let live = tmp.path().join("live");
        for (root, text) in [(&src, "new"), (&live, "old")] {
            fs::create_dir_all(root.join("volumes/functions/notify")).unwrap();
            fs::write(root.join("volumes/functions/notify/index.ts"), text).unwrap()
        }
        let paths = vec![
            "volumes/functions/notify/index.ts".into(),
            "volumes/functions/../outside".into(),
            "README.md".into(),
        ];
        let sha = "a".repeat(40);
        let mut host = MockFunctions {
            failed: true,
            ..Default::default()
        };
        assert!(sync_functions(&src, &live, &paths, &sha, &mut host).is_err());
        assert_eq!(
            fs::read_to_string(live.join("volumes/functions/notify/index.ts")).unwrap(),
            "old"
        );
        assert_eq!(host.restarts, 2);
        assert_eq!(
            sync_functions(&src, &live, &paths, &sha, &mut host).unwrap(),
            vec!["volumes/functions/notify/index.ts"]
        );
        assert_eq!(
            fs::read_to_string(live.join("volumes/functions/notify/index.ts")).unwrap(),
            "new"
        )
    }
    #[test]
    fn visible_state_omits_rollback_secrets() {
        let value = visible_state(
            json!({"components":{"web":{"sha":"abc","rollback_units":{"unit":"SECRET"}}}}),
        );
        assert!(value["components"]["web"].get("rollback_units").is_none())
    }
}

pub mod agent;
