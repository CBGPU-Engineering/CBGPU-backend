use cbgpu_admin_ops::*;
use std::{env, path::PathBuf};
fn run() -> Result<()> {
    let args: Vec<_> = env::args().skip(1).collect();
    if args.is_empty() || args == ["--help"] {
        println!(
            "cbgpu-admin-ops check-email-domain|build-frontend|verify-backend|verify-push-credentials|verify-recovery-simulator|verify-server-runtimes|restart-controller-preserving-gpus [--apply] [--root PATH] [--frontend PATH] [--allow-test-email] [--allow-controller-restart]\nHistorical activate-local-backend, final-sync and configure-scheduled-jobs are permanently retired."
        );
        return Ok(());
    }
    let command = &args[0];
    let summary = plan(command)?;
    let (mut apply, mut allow_email, mut allow_controller) = (false, false, false);
    let mut root = env::var_os("CBGPU_BACKEND_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/home/cbgpu/CBGPU-live/backend"));
    let mut frontend = None;
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--apply" => apply = true,
            "--allow-test-email" => allow_email = true,
            "--allow-controller-restart" => allow_controller = true,
            "--root" | "--frontend" => {
                let flag = &args[i];
                i += 1;
                let value = PathBuf::from(args.get(i).ok_or("Missing option value")?);
                if flag == "--root" {
                    root = value
                } else {
                    frontend = Some(value)
                }
            }
            _ => return Err("Unknown operator option".into()),
        }
        i += 1;
    }
    if !apply {
        println!("{summary}");
        return Ok(());
    }
    if summary["retired"] == true {
        return Err("Historical cutover is retired: existing users/projects cannot be overwritten by replaying the old source database".into());
    }
    let mut io = SystemOperator;
    let result = match command.as_str() {
        "check-email-domain" => check_email_domain(&root, &mut io),
        "build-frontend" => build_frontend(
            &root,
            &frontend.ok_or("--frontend is required for an explicit build checkout")?,
            &mut io,
        ),
        "verify-backend" => verify_backend(&root, &mut io),
        "verify-push-credentials" => verify_push(&root, &mut io),
        "verify-recovery-simulator" => verify_recovery(&root, allow_email, &mut io),
        "restart-controller-preserving-gpus" => restart_controller(
            &PathBuf::from("/run/cbgpu-daemon-preserve-idle-once"),
            allow_controller,
            &mut io,
        ),
        "verify-server-runtimes" => verify_server_runtimes(&mut io),
        _ => Err("Unknown operation".into()),
    }?;
    println!("{result}");
    Ok(())
}
fn main() {
    unsafe {
        libc::umask(0o077);
    }
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1)
    }
}
