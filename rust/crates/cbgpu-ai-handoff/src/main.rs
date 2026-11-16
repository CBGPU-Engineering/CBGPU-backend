use cbgpu_ai_handoff::{ImportOptions, Result};
use std::{collections::BTreeMap, path::PathBuf};
#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("AI handoff refused: {error}");
        std::process::exit(1)
    }
}
async fn run() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let action = args.next().unwrap_or("help".into());
    if ["help", "--help"].contains(&action.as_str()) {
        println!(
            "cbgpu-ai-handoff preflight --state PATH [--legacy-unit cbgpu-ai.service] [--native-unit cbgpu-ai-native.service]\nimport-drained additionally requires --apply --confirm IMPORT_DRAINED_PRIVATE_AI_AFTER_REVIEW --backup NEW_PATH --database-url-file PRIVATE_FILE [--bulk PATH]. Both units must be stopped and legacy runtime-masked. Schema must already be provisioned. No service activation is performed.\nrollback-check requires --state PATH --backup PATH --database-url-file PRIVATE_FILE; it only checks eligibility, never changes a database or service."
        );
        return Ok(());
    }
    let mut flags = BTreeMap::new();
    let mut apply = false;
    while let Some(flag) = args.next() {
        if flag == "--apply" {
            apply = true;
            continue;
        }
        if ![
            "--state",
            "--bulk",
            "--backup",
            "--database-url-file",
            "--legacy-unit",
            "--native-unit",
            "--confirm",
        ]
        .contains(&flag.as_str())
        {
            return Err("Unknown option".into());
        }
        if flags
            .insert(flag, args.next().ok_or("Option value missing")?)
            .is_some()
        {
            return Err("Duplicate option".into());
        }
    }
    let state = PathBuf::from(flags.get("--state").ok_or("Explicit state path required")?);
    let legacy = flags
        .get("--legacy-unit")
        .map(String::as_str)
        .unwrap_or("cbgpu-ai.service");
    let native = flags
        .get("--native-unit")
        .map(String::as_str)
        .unwrap_or("cbgpu-ai-native.service");
    let result = if action == "preflight" {
        if apply {
            return Err("Preflight cannot apply changes".into());
        }
        cbgpu_ai_handoff::preflight(&state, legacy, native).await?
    } else {
        let file = PathBuf::from(
            flags
                .get("--database-url-file")
                .ok_or("Private database URL file required")?,
        );
        use std::os::unix::fs::PermissionsExt;
        let meta = std::fs::symlink_metadata(&file)?;
        if !meta.is_file()
            || meta.file_type().is_symlink()
            || meta.permissions().mode() & 0o077 != 0
        {
            return Err("Database URL must be a private regular file (0600)".into());
        }
        let database = std::fs::read_to_string(file)?;
        let backup = PathBuf::from(flags.get("--backup").ok_or("Backup path required")?);
        if action == "rollback-check" {
            if apply {
                return Err("Rollback check never applies changes".into());
            }
            cbgpu_ai_handoff::rollback_check(&state, &backup, database.trim()).await?
        } else if action == "import-drained" {
            if !apply {
                return Err(
                    "Import requires explicit --apply after review; use preflight first".into(),
                );
            }
            let bulk = flags.get("--bulk").map(PathBuf::from);
            cbgpu_ai_handoff::import_drained(ImportOptions {
                state: &state,
                bulk: bulk.as_deref(),
                backup: &backup,
                database: database.trim(),
                legacy,
                native,
                confirmation: flags.get("--confirm").map(String::as_str).unwrap_or(""),
            })
            .await?
        } else {
            return Err("Unknown action".into());
        }
    };
    println!("{}", serde_json::to_string(&result)?);
    Ok(())
}
