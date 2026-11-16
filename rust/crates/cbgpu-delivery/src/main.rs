use cbgpu_delivery::{
    Result,
    agent::{Agent, Config, SystemRuntime},
    atomic, candidate, changed_paths, github, lock, plan_backend, read_json, valid_sha,
    visible_state,
};
use serde_json::{Value, json};
use std::{env, path::PathBuf};
fn run() -> Result<()> {
    let args: Vec<_> = env::args().skip(1).collect();
    let home = PathBuf::from(env::var_os("HOME").ok_or("HOME is unset")?);
    let root = env::var_os("CBGPU_DELIVERY_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".local/share/cbgpu-delivery"));
    if args == ["--status"] {
        let mut state = read_json(&root.join("state.json"))?;
        if state.is_null() {
            state = json!({"components":{},"paused":false})
        }
        println!(
            "{}",
            serde_json::to_string_pretty(&visible_state(state))
                .map_err(|_| "Cannot encode status")?
        );
        return Ok(());
    }
    if args == ["--capabilities"] {
        println!(
            "{}",
            json!({"status":true,"ci_gate":true,"policy_plan":true,"transaction_core":true,"public_chunk_retention":true,"live_apply":true,"service_handoff":"Explicit native service adapters required; no automatic data conversion"})
        );
        return Ok(());
    }
    let _lock = lock(&root)?;
    let config: Config = serde_json::from_value(read_json(&root.join("config.json"))?)
        .map_err(|_| "Invalid private delivery config")?;
    let mut state = read_json(&root.join("state.json"))?;
    if state.is_null() {
        state = json!({"components":{},"paused":false})
    }
    if args == ["--pause"] || args == ["--resume"] {
        state["paused"] = Value::Bool(args[0] == "--pause");
        atomic(
            &root.join("state.json"),
            &serde_json::to_vec_pretty(&state).map_err(|_| "Cannot encode state")?,
        )?;
        return Ok(());
    }
    if args.len() == 2 && args[0] == "--plan" {
        let component = &args[1];
        if !["web", "backend"].contains(&component.as_str()) {
            return Err("Unknown component".into());
        }
        let repo = config
            .repositories
            .get(component)
            .ok_or("Repository not configured")?;
        let (sha, ci) = candidate(repo)?;
        let old = state["components"][component]["sha"].as_str();
        let paths = if let Some(old) = old {
            if !valid_sha(old) {
                return Err("Invalid previous commit".into());
            }
            changed_paths(&github(&format!("repos/{repo}/compare/{old}...{sha}"))?)?
        } else {
            Vec::new()
        };
        let policy = if component == "backend" {
            plan_backend(
                &paths,
                state["components"][component]["approved_infrastructure_sha"] == sha,
                cbgpu_delivery::agent::busy_backend(&config.runtime_backend)?,
            )?
        } else {
            json!({"status":"validated_policy","apply_supported":true})
        };
        println!(
            "{}",
            json!({"component":component,"commit":sha,"ci":ci,"paths":paths,"policy":policy})
        );
        return Ok(());
    }
    let (mut only, mut retry, mut rollback, mut approve) = (None, None, None, None);
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--once" => (),
            "--only" | "--retry" | "--rollback" | "--approve-backend" => {
                let kind = args[i].as_str();
                i += 1;
                let value = args.get(i).ok_or("Missing flag value")?.clone();
                match kind {
                    "--only" => only = Some(value),
                    "--retry" => retry = Some(value),
                    "--rollback" => rollback = Some(value),
                    _ => approve = Some(value),
                }
            }
            _ => return Err("Unknown delivery option".into()),
        }
        i += 1;
    }
    if only
        .as_deref()
        .is_some_and(|v| !["web", "backend"].contains(&v))
    {
        return Err("Unknown component".into());
    }
    let mut agent = Agent {
        root,
        settings: home.join(".config/systemd/user"),
        config,
        state,
        runtime: SystemRuntime,
    };
    if let Some(sha) = approve {
        agent.approve_backend(&sha)?;
    }
    if let Some(component) = rollback {
        return agent.rollback(&component);
    }
    if let Some(component) = &retry {
        agent.retry(component)?;
    }
    if agent.state["paused"] == true {
        return Ok(());
    }
    agent.recover()?;
    agent.poll(only.as_deref().or(retry.as_deref()))
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
