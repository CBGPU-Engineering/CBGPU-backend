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
fn private_dir(path: &Path) -> Result<()> {
    safe_path(path)?;
    fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let m = fs::metadata(path)?;
        if m.uid() != unsafe { libc::geteuid() } {
            return Err("Directory belongs to another user".into());
        }
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}
pub fn prepare_identity_parent(path: &Path) -> Result<()> {
    safe_path(path)?;
    private_dir(path.parent().ok_or("Missing identity directory")?)
}
pub struct Plan {
    pub unit: PathBuf,
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
        binary,
        identity: identity.into(),
        text,
        bytes,
    })
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
    let (ok, state) = commands.output(
        "systemctl",
        &[
            "--user",
            "show",
            "cbgpu-node-inventory.service",
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
    private_dir(plan.unit.parent().ok_or("Missing unit parent")?)?;
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
        vec!["--user", "enable", "--now", "cbgpu-node-inventory.service"],
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
                true,
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
}
