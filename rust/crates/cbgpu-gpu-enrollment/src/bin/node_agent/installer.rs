//! Explicit user-service installation only; never changes root controllers or drivers.
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    error::Error,
    fs,
    path::{Path, PathBuf},
    process::Command,
};
type Result<T> = std::result::Result<T, Box<dyn Error>>;
pub trait Commands {
    fn output(&self, program: &str, args: &[&str]) -> Result<(bool, String)>;
}
pub struct RealCommands;
impl Commands for RealCommands {
    fn output(&self, program: &str, args: &[&str]) -> Result<(bool, String)> {
        let program = if program == "/usr/bin/nvidia-smi" {
            cbgpu_gpu_enrollment::nvidia_path::trusted()?
        } else {
            program
        };
        let o = Command::new(program).args(args).output()?;
        if o.stdout.len() > 65536 {
            return Err("Diagnostic output too large".into());
        }
        Ok((o.status.success(), String::from_utf8(o.stdout)?))
    }
}
pub fn diagnose(os_release: &str, arch: &str, commands: &impl Commands) -> Value {
    let ubuntu = os_release
        .lines()
        .any(|l| matches!(l, "ID=ubuntu" | "ID=\"ubuntu\""));
    let architecture = match arch {
        "x86_64" => "AMD64",
        "aarch64" => "ARM64",
        _ => "UNSUPPORTED",
    };
    let nvidia = ubuntu
        && commands
            .output(
                "/usr/bin/nvidia-smi",
                &["--query-gpu=index", "--format=csv,noheader"],
            )
            .is_ok_and(|(ok, s)| ok && !s.trim().is_empty());
    let user_systemd = commands
        .output("systemctl", &["--user", "show-environment"])
        .is_ok_and(|(ok, _)| ok);
    #[cfg(unix)]
    let uid = unsafe { libc::geteuid() }.to_string();
    #[cfg(not(unix))]
    let uid = String::new();
    let lingering = commands
        .output(
            "loginctl",
            &["show-user", &uid, "--property=Linger", "--value"],
        )
        .ok()
        .and_then(|(ok, value)| {
            if ok {
                match value.trim() {
                    "yes" => Some(true),
                    "no" => Some(false),
                    _ => None,
                }
            } else {
                None
            }
        });
    json!({"user_lingering":lingering,"boot_continuity_verified":false,"persistence_prerequisites_ready":user_systemd&&lingering==Some(true),"ubuntu":ubuntu,"architecture":architecture,"binary_architecture":std::env::consts::ARCH,"existing_nvidia_driver_ready":nvidia,"user_systemd_ready":user_systemd,"inventory_only":true,"execution_ready":false,"drivers_changed":false,"artifact_note":"This installed executable is the only verified artifact; ARM64 requires an independently built and verified ARM64 executable."})
}
fn safe_path(path: &Path) -> Result<()> {
    if !path.is_absolute()
        || path.to_str().is_none_or(|s| {
            s.chars()
                .any(|c| c.is_control() || matches!(c, '%' | '"' | '\\'))
        })
    {
        return Err("Use an absolute path without systemd metacharacters".into());
    }
    for part in path.ancestors() {
        if let Ok(m) = fs::symlink_metadata(part)
            && m.file_type().is_symlink()
        {
            return Err("Symlink path rejected".into());
        }
    }
    Ok(())
}
fn ensure_owned_dir(path: &Path, private: bool) -> Result<()> {
    safe_path(path)?;
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    // DirBuilder preserves every pre-existing directory's permissions.
    builder.create(path)?;
    let m = fs::symlink_metadata(path)?;
    if !m.is_dir() {
        return Err("Expected an owned regular directory".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        if m.uid() != unsafe { libc::geteuid() } {
            return Err("Directory belongs to another user".into());
        }
        if private && m.permissions().mode() & 0o077 != 0 {
            return Err("Existing identity/binary directory must already be private; permissions were not changed".into());
        }
    }
    #[cfg(not(unix))]
    let _ = private;
    Ok(())
}
fn private_dir(path: &Path) -> Result<()> {
    ensure_owned_dir(path, true)
}
pub fn executor_lock(root: &Path) -> std::result::Result<std::fs::File, &'static str> {
    private_dir(root).map_err(|_| "PRIVATE_FILE_INVALID")?;
    let mut o = fs::OpenOptions::new();
    o.create(true).read(true).write(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    }
    let f = o
        .open(root.join(".executor.lock"))
        .map_err(|_| "EXECUTOR_ALREADY_RUNNING")?;
    fs2::FileExt::try_lock_exclusive(&f).map_err(|_| "EXECUTOR_ALREADY_RUNNING")?;
    Ok(f)
}
pub fn prepare_identity_parent(path: &Path) -> Result<()> {
    safe_path(path)?;
    private_dir(path.parent().ok_or("Missing identity directory")?)
}
pub struct Plan {
    pub unit: PathBuf,
    pub service: String,
    pub binary: PathBuf,
    pub identity: PathBuf,
    pub text: String,
    pub bytes: Vec<u8>,
}
pub fn plan(home: &Path, executable: &Path, identity: &Path) -> Result<Plan> {
    safe_path(home)?;
    safe_path(executable)?;
    safe_path(identity)?;
    if !identity.starts_with(home.join(".config/cbgpu-node")) {
        return Err("User service identity must be inside ~/.config/cbgpu-node".into());
    }
    let bytes = fs::read(executable)?;
    if !bytes.starts_with(b"\x7fELF") {
        return Err("Only the current verified native ELF may be installed".into());
    }
    let digest = format!("{:x}", Sha256::digest(&bytes));
    let binary = home
        .join(".local/share/cbgpu-node-agent")
        .join(format!("node-agent-{digest}"));
    let unit = home.join(".config/systemd/user/cbgpu-node-inventory.service");
    let text = format!(
        "[Unit]\nDescription=CBGPU signed inventory reporter (not an executor)\nAfter=network-online.target\nWants=network-online.target\n\n[Service]\nType=simple\nExecStart=\"{}\" run \"{}\"\nRestart=on-failure\nRestartSec=30\nUMask=0077\nNoNewPrivileges=yes\nPrivateTmp=yes\nProtectSystem=strict\nProtectHome=read-only\nReadWritePaths=\"{}\"\nRestrictAddressFamilies=AF_INET AF_INET6 AF_UNIX\n\n[Install]\nWantedBy=default.target\n",
        binary.display(),
        identity.display(),
        identity.parent().ok_or("Missing parent")?.display()
    );
    Ok(Plan {
        unit,
        service: "cbgpu-node-inventory.service".into(),
        binary,
        identity: identity.into(),
        text,
        bytes,
    })
}
pub fn executor_plan(
    home: &Path,
    executable: &Path,
    identity: &Path,
    policy: &Path,
) -> Result<Plan> {
    safe_path(policy)?;
    let mut plan = plan(home, executable, identity)?;
    plan.service = "cbgpu-node-executor.service".into();
    plan.unit = home.join(".config/systemd/user").join(&plan.service);
    plan.text = plan
        .text
        .replace(
            "Description=CBGPU signed inventory reporter (not an executor)",
            "Description=CBGPU explicitly approved temporary notebook node",
        )
        .replace(
            &format!("run \"{}\"", identity.display()),
            &format!(
                "run \"{}\" --notebooks \"{}\"",
                identity.display(),
                policy.display()
            ),
        );
    Ok(plan)
}
fn create_private(path: &Path, bytes: &[u8], executable: bool) -> Result<()> {
    use std::io::Write;
    let mut o = fs::OpenOptions::new();
    o.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(if executable { 0o700 } else { 0o600 })
            .custom_flags(libc::O_NOFOLLOW);
    }
    let mut f = o.open(path)?;
    f.write_all(bytes)?;
    f.sync_all()?;
    Ok(())
}
pub fn apply(plan: &Plan, commands: &impl Commands) -> Result<()> {
    safe_path(&plan.unit)?;
    safe_path(&plan.binary)?;
    safe_path(&plan.identity)?;
    if ![
        "cbgpu-node-inventory.service",
        "cbgpu-node-executor.service",
    ]
    .contains(&plan.service.as_str())
        || plan.unit.file_name().and_then(|v| v.to_str()) != Some(plan.service.as_str())
    {
        return Err("Unrecognized node service".into());
    }
    if plan.service == "cbgpu-node-executor.service" {
        for check in ["is-active", "is-enabled"] {
            if commands
                .output(
                    "systemctl",
                    &["--user", check, "--quiet", "cbgpu-node-inventory.service"],
                )?
                .0
            {
                return Err("Explicitly stop and disable only your previous inventory user service before installing notebook mode".into());
            }
        }
    }
    let (ok, state) = commands.output(
        "systemctl",
        &[
            "--user",
            "show",
            &plan.service,
            "--property=LoadState",
            "--value",
        ],
    )?;
    if !ok || !matches!(state.trim(), "not-found" | "") {
        return Err("Existing or uninspectable inventory service: no replacement allowed".into());
    }
    if plan.unit.exists() {
        return Err("Unit already exists; no replacement allowed".into());
    }
    private_dir(plan.identity.parent().ok_or("Missing identity parent")?)?;
    ensure_owned_dir(plan.unit.parent().ok_or("Missing unit parent")?, false)?;
    private_dir(plan.binary.parent().ok_or("Missing binary parent")?)?;
    if plan.binary.exists() {
        if fs::read(&plan.binary)? != plan.bytes {
            return Err("Existing executable differs".into());
        }
    } else {
        create_private(&plan.binary, &plan.bytes, true)?;
    }
    create_private(&plan.unit, plan.text.as_bytes(), false)?;
    for args in [
        vec!["--user", "daemon-reload"],
        vec!["--user", "enable", "--now", plan.service.as_str()],
    ] {
        if !commands.output("systemctl", &args)?.0 {
            return Err("Service setup incomplete; private files retained for explicit inspection, no other service changed".into());
        }
    }
    Ok(())
}
pub fn retry_seconds(failures: u32) -> u64 {
    (5u64.saturating_mul(1u64 << failures.min(6))).min(300)
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        cell::RefCell,
        time::{SystemTime, UNIX_EPOCH},
    };
    struct Fake {
        calls: RefCell<Vec<String>>,
        existing: bool,
    }
    impl Commands for Fake {
        fn output(&self, p: &str, a: &[&str]) -> Result<(bool, String)> {
            self.calls.borrow_mut().push(format!("{p} {}", a.join(" ")));
            Ok((
                !a.contains(&"--quiet"),
                if a.contains(&"--property=LoadState") {
                    if self.existing { "loaded" } else { "not-found" }
                } else {
                    "0"
                }
                .into(),
            ))
        }
    }
    fn tmp() -> PathBuf {
        std::env::temp_dir().join(format!(
            "cbgpu-installer-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }
    #[test]
    fn diagnostics_do_not_change_driver_or_services() {
        let f = Fake {
            calls: RefCell::new(vec![]),
            existing: false,
        };
        let d = diagnose("ID=ubuntu", "aarch64", &f);
        assert_eq!(d["architecture"], "ARM64");
        assert_eq!(d["execution_ready"], false);
        assert!(
            f.calls
                .borrow()
                .iter()
                .all(|c| !c.contains("restart") && !c.contains("enable") && !c.contains("install"))
        );
        assert!(
            !diagnose("ID=debian", "x86_64", &f)["ubuntu"]
                .as_bool()
                .unwrap()
        );
    }
    #[test]
    fn explicit_install_is_private_and_never_replaces_units() {
        let d = tmp();
        fs::create_dir_all(&d).unwrap();
        let e = d.join("elf");
        fs::write(&e, b"\x7fELFfixture").unwrap();
        let id = d.join(".config/cbgpu-node/identity.json");
        let p = plan(&d, &e, &id).unwrap();
        assert!(!p.unit.exists());
        let f = Fake {
            calls: RefCell::new(vec![]),
            existing: false,
        };
        apply(&p, &f).unwrap();
        assert!(p.unit.exists());
        assert!(p.text.contains("inventory reporter"));
        assert!(!p.text.contains("sudo"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&p.unit).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        let original = fs::read(&p.unit).unwrap();
        assert!(apply(&p, &f).is_err());
        assert_eq!(fs::read(&p.unit).unwrap(), original);
        fs::remove_dir_all(d).unwrap();
    }
    #[test]
    fn existing_loaded_service_is_never_touched() {
        let d = tmp();
        fs::create_dir_all(&d).unwrap();
        let e = d.join("elf");
        fs::write(&e, b"\x7fELFfixture").unwrap();
        let p = plan(&d, &e, &d.join(".config/cbgpu-node/identity.json")).unwrap();
        let f = Fake {
            calls: RefCell::new(vec![]),
            existing: true,
        };
        assert!(apply(&p, &f).is_err());
        assert!(!p.unit.exists());
        assert_eq!(f.calls.borrow().len(), 1);
        fs::remove_dir_all(d).unwrap();
    }
    #[test]
    fn paths_and_backoff_are_bounded() {
        assert!(safe_path(Path::new("/tmp/%h/key")).is_err());
        assert!(safe_path(Path::new("relative")).is_err());
        assert_eq!(retry_seconds(0), 5);
        assert_eq!(retry_seconds(100), 300);
    }
    #[cfg(unix)]
    #[test]
    fn existing_public_directory_is_never_chmodded_for_identity() {
        use std::os::unix::fs::PermissionsExt;
        let d = tmp();
        fs::create_dir(&d).unwrap();
        fs::set_permissions(&d, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(prepare_identity_parent(&d.join("identity.json")).is_err());
        assert_eq!(
            fs::metadata(&d).unwrap().permissions().mode() & 0o777,
            0o755
        );
        assert!(!d.join("identity.json").exists());
        fs::remove_dir(&d).unwrap();
    }
    #[cfg(unix)]
    #[test]
    fn existing_user_unit_directory_permissions_are_preserved() {
        use std::os::unix::fs::PermissionsExt;
        let d = tmp();
        let units = d.join(".config/systemd/user");
        fs::create_dir_all(&units).unwrap();
        fs::set_permissions(&units, fs::Permissions::from_mode(0o755)).unwrap();
        let e = d.join("elf");
        fs::write(&e, b"\x7fELFfixture").unwrap();
        let identity = d.join(".config/cbgpu-node/identity.json");
        prepare_identity_parent(&identity).unwrap();
        let p = plan(&d, &e, &identity).unwrap();
        let f = Fake {
            calls: RefCell::new(vec![]),
            existing: false,
        };
        apply(&p, &f).unwrap();
        assert_eq!(
            fs::metadata(&units).unwrap().permissions().mode() & 0o777,
            0o755
        );
        assert_eq!(
            fs::metadata(identity.parent().unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(p.binary.parent().unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        fs::remove_dir_all(d).unwrap();
    }
    #[test]
    fn executor_service_is_explicit_separate_and_uses_no_privilege_grant() {
        let d = tmp();
        fs::create_dir_all(&d).unwrap();
        let e = d.join("elf");
        fs::write(&e, b"\x7fELFfixture").unwrap();
        let p = executor_plan(
            &d,
            &e,
            &d.join(".config/cbgpu-node/identity.json"),
            &d.join("policy.json"),
        )
        .unwrap();
        assert_eq!(p.service, "cbgpu-node-executor.service");
        assert!(p.text.contains("--notebooks"));
        assert!(!p.text.contains("sudo"));
        assert!(!p.text.contains("SupplementaryGroups"));
        let f = Fake {
            calls: RefCell::new(vec![]),
            existing: false,
        };
        apply(&p, &f).unwrap();
        assert!(
            !d.join(".config/systemd/user/cbgpu-node-inventory.service")
                .exists()
        );
        assert!(
            f.calls
                .borrow()
                .iter()
                .all(|c| !c.contains(" stop ") && !c.contains(" disable "))
        );
        fs::remove_dir_all(d).unwrap();
    }
}
