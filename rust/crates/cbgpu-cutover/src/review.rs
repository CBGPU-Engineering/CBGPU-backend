//! Read-only evidence collection. Never obtains the mutation lock or invokes Docker/netctl.
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    process::Command,
};
type Result<T> = std::result::Result<T, String>;
fn read(path: &Path, max: u64) -> Result<Vec<u8>> {
    let meta =
        std::fs::symlink_metadata(path).map_err(|_| "Required review evidence is unavailable")?;
    if !meta.is_file() || meta.file_type().is_symlink() || meta.len() > max {
        return Err("Invalid review evidence file".into());
    }
    std::fs::read(path).map_err(|_| "Cannot read review evidence".into())
}
fn output(program: &str, args: &[&std::ffi::OsStr]) -> Result<String> {
    let out = Command::new("timeout")
        .args(["--signal=TERM", "--kill-after=1s", "5s", program])
        .env("GIT_OPTIONAL_LOCKS", "0")
        .args(args)
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|_| "Read-only review command unavailable")?;
    if !out.status.success() || out.stdout.len() > 1024 * 1024 {
        return Err("Read-only review command failed".into());
    }
    String::from_utf8(out.stdout)
        .map(|s| s.trim().to_owned())
        .map_err(|_| "Invalid review command response".into())
}
fn digest(path: &Path) -> Result<String> {
    let meta = std::fs::symlink_metadata(path).map_err(|_| "Evidence digest source unavailable")?;
    if !meta.is_file() || meta.file_type().is_symlink() || meta.len() > 512 * 1024 * 1024 {
        return Err("Invalid digest source".into());
    }
    let text = output("sha256sum", &["--".as_ref(), path.as_os_str()])?;
    let hash = text
        .split_whitespace()
        .next()
        .ok_or("Missing evidence digest")?;
    if hash.len() != 64 || !hash.bytes().all(|v| v.is_ascii_hexdigit()) {
        return Err("Invalid evidence digest".into());
    }
    Ok(hash.to_owned())
}
fn git(path: &Path, args: &[&str]) -> Result<String> {
    let mut values = vec![
        "-c".as_ref(),
        "core.fsmonitor=false".as_ref(),
        "-C".as_ref(),
        path.as_os_str(),
    ];
    values.extend(args.iter().map(std::ffi::OsStr::new));
    output("git", &values)
}
fn repo(path: &Path) -> Result<Value> {
    let revision = git(path, &["rev-parse", "--verify", "HEAD"])?;
    if revision.len() != 40 || !revision.bytes().all(|v| v.is_ascii_hexdigit()) {
        return Err("Invalid source revision".into());
    }
    Ok(
        json!({"revision":revision,"dirty":!git(path,&["status","--porcelain","--untracked-files=normal"])?.is_empty()}),
    )
}
fn file_name(value: &str) -> bool {
    !value.is_empty()
        && value.ends_with(".sql")
        && value
            .bytes()
            .all(|v| v.is_ascii_alphanumeric() || matches!(v, b'_' | b'.' | b'-'))
        && !value.contains("..")
}
fn unit(name: &str, user: bool) -> Value {
    let mut arguments = vec![];
    if user {
        arguments.push("--user".as_ref());
    }
    arguments.extend(
        [
            "show",
            name,
            "--property=ActiveState",
            "--property=MainPID",
            "--property=FragmentPath",
            "--property=LoadState",
        ]
        .into_iter()
        .map(std::ffi::OsStr::new),
    );
    let response = output("systemctl", &arguments);
    let Ok(text) = response else {
        return json!({"unit":name,"scope":if user{"user"}else{"system"},"observation":"UNVERIFIED"});
    };
    if text.len() > 16384 {
        return json!({"unit":name,"observation":"UNVERIFIED"});
    }
    let fields = text
        .lines()
        .filter_map(|line| {
            line.split_once('=')
                .map(|(k, v)| (k.to_owned(), v.to_owned()))
        })
        .collect::<BTreeMap<_, _>>();
    let pid = fields
        .get("MainPID")
        .and_then(|p| p.parse::<u32>().ok())
        .unwrap_or(0);
    let mut sources = vec![];
    if pid > 0
        && let Ok(bytes) = read(&PathBuf::from(format!("/proc/{pid}/cmdline")), 65536)
    {
        for argument in bytes.split(|v| *v == 0) {
            if let Ok(value) = std::str::from_utf8(argument) {
                let path = Path::new(value);
                let name = path.file_name().and_then(|v| v.to_str()).unwrap_or("");
                if path.is_absolute()
                    && (name.ends_with(".py")
                        || name.ends_with(".mts")
                        || name.ends_with(".js")
                        || [
                            "cbgpu-ai",
                            "cbgpu-gpu-gateway",
                            "cbgpu-gpu-controller",
                            "cbgpu-cpu-controller",
                            "cbgpu-gateway",
                            "cbgpu-materials",
                            "cbgpu-latex",
                        ]
                        .contains(&name))
                {
                    sources.push(json!({"path":value,"sha256":digest(path).ok(),"release_revision":path.components().filter_map(|c|c.as_os_str().to_str()).find(|v|v.len()==40&&v.bytes().all(|b|b.is_ascii_hexdigit()))}));
                }
            }
        }
    }
    json!({"unit":name,"scope":if user{"user"}else{"system"},"active":fields.get("ActiveState"),"load":fields.get("LoadState"),"pid":pid,"fragment":fields.get("FragmentPath"),"recognized_source_files":sources,"writer_fence_proven":false})
}
fn container(name: &str) -> Value {
    let result = output(
        "docker",
        &[
            "inspect".as_ref(),
            "--format".as_ref(),
            "{{json .Config.Image}} {{json .Image}} {{json .State.Running}} {{json .State.Pid}}"
                .as_ref(),
            name.as_ref(),
        ],
    );
    let Ok(text) = result else {
        return json!({"kind":"container","name":name,"observation":"UNVERIFIED","writer_fence_proven":false});
    };
    let values = serde_json::Deserializer::from_str(&text)
        .into_iter::<Value>()
        .collect::<std::result::Result<Vec<_>, _>>();
    match values {
        Ok(v) if v.len() == 4 => {
            json!({"kind":"container","name":name,"image":v[0],"image_id":v[1],"running":v[2],"host_pid":v[3],"source_revision":"UNVERIFIED","writer_fence_proven":false})
        }
        _ => json!({"kind":"container","name":name,"observation":"UNVERIFIED"}),
    }
}
pub fn generate(repos: &BTreeMap<String, PathBuf>, proof: &Path, inspect: bool) -> Result<Value> {
    let backend = repos.get("backend").ok_or("Backend source is required")?;
    let mut revisions = BTreeMap::new();
    for (name, path) in repos {
        revisions.insert(name.clone(), repo(path)?);
    }
    let migrations = backend.join("supabase/migrations");
    let mut inventory = BTreeMap::new();
    for entry in std::fs::read_dir(&migrations).map_err(|_| "Migration source directory missing")? {
        let entry = entry.map_err(|_| "Cannot enumerate migration source")?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| "Invalid migration filename")?;
        if file_name(&name) {
            inventory.insert(name, digest(&entry.path())?);
        }
    }
    let hashes: Vec<Value> = serde_json::from_slice(&read(
        &proof.join("migration-hashes.json"),
        2 * 1024 * 1024,
    )?)
    .map_err(|_| "Invalid frozen migration inventory")?;
    if hashes.is_empty() {
        return Err("Empty frozen migration inventory".into());
    }
    let passed = String::from_utf8(read(&proof.join("PASSED"), 4096)?)
        .map_err(|_| "Invalid clone proof marker")?;
    let marker =
        passed.contains("migrations replayed twice") && passed.contains("no production DDL");
    let baseline = read(&proof.join("before.csv"), 1024 * 1024)?
        == read(&proof.join("after.csv"), 1024 * 1024)?;
    let mut verified = vec![];
    let mut seen = std::collections::BTreeSet::new();
    let mut all = true;
    for entry in hashes {
        let name = entry["file"].as_str().ok_or("Invalid clone filename")?;
        if !seen.insert(name.to_owned()) {
            return Err("Duplicate frozen migration inventory item".into());
        }
        let expected = entry["sha256"].as_str().ok_or("Invalid clone digest")?;
        if !file_name(name)
            || expected.len() != 64
            || !expected.bytes().all(|v| v.is_ascii_hexdigit())
        {
            return Err("Invalid clone inventory item".into());
        }
        let current = inventory.get(name);
        let matched = current.is_some_and(|v| v == expected);
        all &= matched;
        verified.push(json!({"file":name,"tested_sha256":expected,"current_sha256":current,"matches":matched}));
    }
    // A newly added candidate must invalidate an older otherwise-matching
    // proof. Historical source migrations outside this explicit rehearsal list
    // remain inventoried, but are not falsely described as newly tested.
    let declaration = String::from_utf8(read(
        &backend.join("rust/crates/cbgpu-schools/src/bin/schema-dry-run.rs"),
        1024 * 1024,
    )?)
    .map_err(|_| "Invalid candidate migration declaration")?;
    let block = declaration
        .split_once("let migrations = [")
        .and_then(|(_, s)| s.split_once("];").map(|(s, _)| s))
        .ok_or("Candidate migration declaration unavailable")?;
    let candidates = block
        .split('"')
        .enumerate()
        .filter(|(index, _)| index % 2 == 1)
        .map(|(_, name)| name.to_owned())
        .collect::<std::collections::BTreeSet<_>>();
    if candidates.is_empty() || candidates.iter().any(|name| !file_name(name)) {
        return Err("Invalid candidate migration declaration".into());
    }
    let candidate_inventory_matches = candidates == seen;
    let unproven_candidates = candidates.difference(&seen).cloned().collect::<Vec<_>>();
    all &= candidate_inventory_matches;
    let observations = if inspect {
        vec![
            unit("cbgpu-daemon.service", false),
            unit("cbgpu-ai.service", true),
            unit("cbgpu-ai-native.service", true),
            unit("cbgpu-deploy.service", true),
            unit("cbgpu-materials.service", true),
            unit("cbgpu-gpu-gateway.service", true),
            unit("cbgpu-gpu-upstream.service", true),
            unit("cbgpu-latex-gateway.service", true),
            unit("cbgpu-latex-compiler.service", true),
            container("cbgpu-materials"),
        ]
    } else {
        vec![]
    };
    Ok(
        json!({"format_version":1,"mode":"activation-review-only","mutations_executed":false,"automatic_activation_supported":false,"ready_for_activation":false,"captured_at_unix":std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_err(|_|"Invalid clock")?.as_secs(),"sources":revisions,"migration_inventory":inventory,"clone":{"passed_marker":marker,"baseline_preserved":baseline,"frozen_source_matches":all,"candidate_inventory_matches":candidate_inventory_matches,"unproven_candidates":unproven_candidates,"checked_migrations":verified},"host_inspection_executed":inspect,"host_observations":observations,"operating_database_migration_state":"UNVERIFIED","orphan_writer_absence":"UNVERIFIED","blockers":["Explicit reviewed production schema/release authorization is still required","Scoped GPU approve intentionally refuses; a reviewed controller ownership/fencing transition is not implemented","Stopped service metadata is not proof of no orphan writers or admission fencing","AI native import requires a genuinely drained and runtime-masked legacy writer plus verified private backup","Cross-school remote inference is not implemented; signed enrollment is inventory only","Rollback after new native writes requires a verified no-new-write fence and compatible reverse import; never restore an old whole database over current user records"]}),
    )
}
pub fn run(args: &[String]) -> Result<Value> {
    let mut repos = BTreeMap::new();
    let mut proof = None;
    let mut inspect = false;
    let mut i = 0;
    while i < args.len() {
        if args[i] == "--inspect-host" {
            if inspect {
                return Err("Duplicate inspection option".into());
            }
            inspect = true;
            i += 1;
            continue;
        }
        let key = args[i].as_str();
        let value = args
            .get(i + 1)
            .ok_or("Review option requires an absolute path")?;
        let path = PathBuf::from(value);
        if !path.is_absolute() {
            return Err("Review paths must be absolute".into());
        }
        if key == "--proof" {
            if proof.replace(path).is_some() {
                return Err("Duplicate clone proof".into());
            }
        } else {
            let name = match key {
                "--repo" => "backend",
                "--web-repo" => "web",
                "--gpu-repo" => "gpu",
                "--latex-repo" => "latex",
                "--materials-repo" => "materials",
                _ => return Err("Unknown review option".into()),
            };
            if repos.insert(name.into(), path).is_some() {
                return Err("Duplicate source repository".into());
            }
        }
        i += 2;
    }
    generate(
        &repos,
        &proof.ok_or("Frozen clone proof is required")?,
        inspect,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Fixture {
        root: PathBuf,
        repos: BTreeMap<String, PathBuf>,
        proof: PathBuf,
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }
    impl Fixture {
        fn new() -> Self {
            static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
            let root = std::env::temp_dir().join(format!(
                "cbgpu-review-fixture-{}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos(),
                NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
            ));
            std::fs::create_dir(&root).unwrap();
            let source = root.join("source");
            let proof = root.join("proof");
            std::fs::create_dir_all(source.join("supabase/migrations")).unwrap();
            std::fs::create_dir(&proof).unwrap();
            assert!(
                Command::new("git")
                    .args(["init", "--quiet"])
                    .arg(&source)
                    .status()
                    .unwrap()
                    .success()
            );
            assert!(
                Command::new("git")
                    .arg("-C")
                    .arg(&source)
                    .args([
                        "-c",
                        "commit.gpgsign=false",
                        "-c",
                        "core.hooksPath=/dev/null",
                        "-c",
                        "user.name=Synthetic",
                        "-c",
                        "user.email=fixture@example.invalid",
                        "commit",
                        "--quiet",
                        "--allow-empty",
                        "-m",
                        "fixture"
                    ])
                    .status()
                    .unwrap()
                    .success()
            );
            let sql = source.join("supabase/migrations/20261117000000_fixture.sql");
            std::fs::write(&sql, "select 1;\n").unwrap();
            let declaration = source.join("rust/crates/cbgpu-schools/src/bin/schema-dry-run.rs");
            std::fs::create_dir_all(declaration.parent().unwrap()).unwrap();
            std::fs::write(
                declaration,
                "let migrations = [\"20261117000000_fixture.sql\"]; ",
            )
            .unwrap();
            std::fs::write(
                proof.join("migration-hashes.json"),
                json!([{"file":"20261117000000_fixture.sql","sha256":digest(&sql).unwrap()}])
                    .to_string(),
            )
            .unwrap();
            std::fs::write(
                proof.join("PASSED"),
                "migrations replayed twice; no production DDL",
            )
            .unwrap();
            for file in ["before.csv", "after.csv"] {
                std::fs::write(proof.join(file), "synthetic,unchanged\n").unwrap();
            }
            Self {
                root,
                repos: BTreeMap::from([("backend".into(), source)]),
                proof,
            }
        }
    }
    #[test]
    fn review_proves_only_frozen_source_and_never_approves_or_creates_operator_state() {
        let f = Fixture::new();
        let before = std::fs::read(f.proof.join("before.csv")).unwrap();
        let result = generate(&f.repos, &f.proof, false).unwrap();
        assert_eq!(result["clone"]["frozen_source_matches"], true);
        assert_eq!(result["clone"]["baseline_preserved"], true);
        assert_eq!(result["mutations_executed"], false);
        assert_eq!(result["automatic_activation_supported"], false);
        assert_eq!(result["ready_for_activation"], false);
        assert_eq!(result["orphan_writer_absence"], "UNVERIFIED");
        assert_eq!(std::fs::read(f.proof.join("before.csv")).unwrap(), before);
        assert!(!f.root.join("operator.lock").exists());
        std::fs::write(
            f.repos["backend"].join("supabase/migrations/20261117000000_fixture.sql"),
            "select 2;",
        )
        .unwrap();
        assert_eq!(
            generate(&f.repos, &f.proof, false).unwrap()["clone"]["frozen_source_matches"],
            false
        );
        std::fs::write(f.proof.join("after.csv"), "changed").unwrap();
        assert_eq!(
            generate(&f.repos, &f.proof, false).unwrap()["clone"]["baseline_preserved"],
            false
        );
    }
    #[test]
    fn adding_a_candidate_invalidates_an_older_matching_proof() {
        let f = Fixture::new();
        let declaration =
            f.repos["backend"].join("rust/crates/cbgpu-schools/src/bin/schema-dry-run.rs");
        std::fs::write(
            declaration,
            "let migrations = [\"20261117000000_fixture.sql\",\"20261117000001_new.sql\"]; ",
        )
        .unwrap();
        let result = generate(&f.repos, &f.proof, false).unwrap();
        assert_eq!(result["clone"]["frozen_source_matches"], false);
        assert_eq!(result["clone"]["candidate_inventory_matches"], false);
        assert_eq!(
            result["clone"]["unproven_candidates"],
            json!(["20261117000001_new.sql"])
        );
    }
    #[test]
    fn review_rejects_path_escape_mutation_options_and_symlinked_evidence() {
        let f = Fixture::new();
        assert!(!file_name("../secret.sql"));
        assert!(!file_name("file/secret.sql"));
        assert!(run(&["--apply".into(), "/tmp".into()]).is_err());
        assert!(run(&["--repo".into(), "relative".into()]).is_err());
        #[cfg(unix)]
        {
            std::fs::remove_file(f.proof.join("PASSED")).unwrap();
            std::os::unix::fs::symlink(f.proof.join("before.csv"), f.proof.join("PASSED")).unwrap();
            assert!(generate(&f.repos, &f.proof, false).is_err());
        }
    }
}
