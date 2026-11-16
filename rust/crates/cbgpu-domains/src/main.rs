use cbgpu_domains::{
    CloudflareApi, DomainError, Settings, ensure_custom_tls, ensure_project, provider_lock,
    remove_custom_tls, remove_project, setup_project_worker, setup_required,
};
use std::{env, path::PathBuf, process::ExitCode};

fn settings(args: &mut Vec<String>) -> Result<Settings, DomainError> {
    let root = if args.first().map(String::as_str) == Some("--root") {
        if args.len() < 3 {
            return Err(DomainError(
                "Usage: cbgpu-domains [--root BACKEND] custom-tls|cloudflare COMMAND [HOST|ID]"
                    .into(),
            ));
        }
        let path = PathBuf::from(args.remove(1));
        args.remove(0);
        path
    } else if let Some(root) = env::var_os("CBGPU_BACKEND_ROOT") {
        PathBuf::from(root)
    } else {
        let cwd = env::current_dir()
            .map_err(|_| DomainError("Backend source directory is unavailable".into()))?;
        if cwd.join("deploy").is_dir() {
            cwd
        } else if cwd.file_name().is_some_and(|name| name == "deploy") {
            cwd.parent()
                .ok_or_else(|| DomainError("Backend source directory is unavailable".into()))?
                .to_path_buf()
        } else {
            return Err(DomainError(
                "Set CBGPU_BACKEND_ROOT or use --root BACKEND".into(),
            ));
        }
    };
    let mut value = Settings::for_root(root);
    if let Some(file) = env::var_os("CBGPU_CLOUDFLARE_TOKEN_FILE") {
        value.token_file = file.into();
    }
    Ok(value)
}
fn usage() -> DomainError {
    DomainError("Usage: cbgpu-domains [--root BACKEND] custom-tls ensure HOST [ID] | custom-tls remove ID | cloudflare setup | cloudflare ensure HOST | cloudflare remove HOST".into())
}
fn execute(args: &[String], configuration: &Settings) -> Result<Option<String>, DomainError> {
    let api = CloudflareApi::from_file(&configuration.token_file)?;
    let mode = args.first().map(String::as_str).ok_or_else(usage)?;
    let command = args.get(1).map(String::as_str).ok_or_else(usage)?;
    let target = args.get(2).map(String::as_str);
    let _capacity_lock = if mode == "custom-tls" {
        Some(provider_lock(&configuration.root)?)
    } else {
        None
    };
    match (mode, command, target, args.len()) {
        ("custom-tls", "ensure", Some(host), 3 | 4) => Ok(Some(
            ensure_custom_tls(&api, configuration, host, args.get(3).map(String::as_str))?
                .to_string(),
        )),
        ("custom-tls", "remove", Some(id), 3) => Ok(Some(
            remove_custom_tls(&api, configuration, id)?.to_string(),
        )),
        ("cloudflare", "setup", None, 2) => {
            setup_project_worker(&api, configuration)?;
            Ok(Some("Project-domain TLS proxy configured. Application builds and runtimes stay on this computer.".into()))
        }
        ("cloudflare", "ensure", Some(host), 3) => {
            ensure_project(&api, configuration, host)?;
            Ok(Some(format!("Hostname registered: {host}")))
        }
        ("cloudflare", "remove", Some(host), 3) => {
            remove_project(&api, configuration, host)?;
            Ok(Some(format!("Hostname removed: {host}")))
        }
        _ => Err(usage()),
    }
}
fn main() -> ExitCode {
    let mut args: Vec<String> = env::args().skip(1).collect();
    if args == ["--help"] || args == ["-h"] {
        println!("{}", usage());
        return ExitCode::SUCCESS;
    }
    if args == ["--version"] {
        println!("cbgpu-domains {}", env!("CARGO_PKG_VERSION"));
        return ExitCode::SUCCESS;
    }
    let custom = args.iter().any(|arg| arg == "custom-tls");
    let result = settings(&mut args).and_then(|configuration| execute(&args, &configuration));
    match result {
        Ok(Some(output)) => {
            println!("{output}");
            ExitCode::SUCCESS
        }
        Ok(None) => ExitCode::SUCCESS,
        // The legacy external-domain adapter deliberately reports provider setup as JSON,
        // exit 0, including for missing/expired provider credentials. Keep that wire contract.
        Err(_) if custom => {
            println!("{}", setup_required());
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}
