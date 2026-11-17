//! Actual disposable PostgreSQL + new private files only; never operational exports.
use cbgpu_deploy_control::{importer, reverse, store};
use serde_json::json;
use sqlx::Row;
use std::os::unix::fs::PermissionsExt;
#[tokio::test]
async fn new_native_writes_reverse_without_overwriting_original_and_reimport_all_state() {
    let Ok(url) = std::env::var("CBGPU_DEPLOY_REVERSE_TEST_DATABASE_URL") else {
        return;
    };
    assert!(url.ends_with("/cbgpu_deploy_reverse_test"));
    let parsed = url::Url::parse(&url).unwrap();
    assert!(matches!(parsed.host_str(), Some("127.0.0.1" | "localhost")));
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect(&url)
        .await
        .unwrap();
    let temp = tempfile::tempdir().unwrap();
    std::fs::set_permissions(temp.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let source = store::Store {
        pool: pool.clone(),
        cipher: cbgpu_deploy_control::crypto::Cipher::new(&[7; 32]).unwrap(),
        data: temp.path().into(),
        free_floor: 0,
    };
    source.migrate().await.unwrap();
    sqlx::query("SET ROLE cbgpu_deploy_control")
        .execute(&pool)
        .await
        .unwrap();
    let options = reverse::Options {
        writers_fenced: true,
        ..Default::default()
    };
    let unfenced = temp.path().join("unfenced.sqlite");
    assert!(
        reverse::export_legacy(&source, &unfenced, &Default::default())
            .await
            .is_err()
    );
    assert!(!unfenced.exists());
    let original = temp.path().join("original.sqlite");
    let old = reverse::export_legacy(&source, &original, &options)
        .await
        .unwrap();
    let old_bytes = std::fs::read(&original).unwrap();
    let owner = "10000000-0000-4000-8000-000000000001";
    let p=source.create(owner,&json!({"slug":"reverse-proof","repository":"https://github.com/example/repo","env":{"TOKEN":"synthetic-private-token"}})).await.unwrap();
    let deployment = source.schedule(&p, owner).await.unwrap();
    let blocked = temp.path().join("active.sqlite");
    assert!(
        reverse::export_legacy(&source, &blocked, &options)
            .await
            .unwrap_err()
            .message
            .contains("Drain")
    );
    assert!(!blocked.exists());
    let work = source
        .claim(&["front_build".into()])
        .await
        .unwrap()
        .unwrap();
    let artifact = temp.path().join("preserved-artifact");
    std::fs::write(&artifact, b"private fixture artifact").unwrap();
    source.finish_work(&work,Some(&json!({"kind":"static","commit":"native-write","directory":temp.path().join(&deployment),"public":temp.path().join(&deployment).join("public")})),None).await.unwrap();
    sqlx::query("INSERT INTO deploy_store.project_members VALUES('frontend',$1,'20000000-0000-4000-8000-000000000002','editor','member@example.invalid','Synthetic member','2026-01-01T00:00:00Z')").bind(&p.id).execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO deploy_store.compose_states(project_id,status,updated_at,access_state,result)VALUES('30000000-0000-4000-8000-000000000003','ready','2026-01-01T00:00:00Z','DENY',$1)").bind(json!({"file":artifact})).execute(&pool).await.unwrap();
    let current = importer::target_fingerprint(&source).await.unwrap();
    assert!(
        reverse::verify_current_bundle(&source, &original, &options)
            .await
            .is_err()
    );
    assert_ne!(old["source_fingerprint"], current);
    assert!(
        reverse::export_legacy(&source, &original, &options)
            .await
            .is_err()
    );
    assert_eq!(std::fs::read(&original).unwrap(), old_bytes);
    let limited = temp.path().join("limited.sqlite");
    let limit = reverse::Options {
        max_rows: 1,
        ..options.clone()
    };
    assert!(
        reverse::export_legacy(&source, &limited, &limit)
            .await
            .is_err()
    );
    assert!(!limited.exists());
    let new_file = temp.path().join("new-legacy.sqlite");
    let proof = reverse::export_legacy(&source, &new_file, &options)
        .await
        .unwrap();
    assert_eq!(proof["source_fingerprint"], current);
    assert_eq!(proof["counts"]["work_items"], 1);
    assert_eq!(
        std::fs::metadata(&new_file).unwrap().permissions().mode() & 0o077,
        0
    );
    let manifest = new_file.with_file_name("new-legacy.sqlite.manifest.json");
    let envelope = std::fs::read_to_string(&manifest).unwrap();
    assert!(!envelope.contains("synthetic-private-token"));
    assert!(!envelope.contains("member@example.invalid"));
    assert_eq!(
        reverse::inspect_bundle(&source, &new_file, &options).unwrap()["source_fingerprint"],
        current
    );
    assert_eq!(
        importer::inspect_read_only(&new_file).await.unwrap()["quiescent"],
        true
    );
    let legacy = sqlx::SqlitePool::connect_with(
        sqlx::sqlite::SqliteConnectOptions::new()
            .filename(&new_file)
            .read_only(true),
    )
    .await
    .unwrap();
    let ciphertext: String = sqlx::query_scalar("SELECT secrets FROM projects WHERE id=?")
        .bind(&p.id)
        .fetch_one(&legacy)
        .await
        .unwrap();
    assert_eq!(ciphertext, p.secrets);
    let member = sqlx::query("SELECT role,email FROM project_members")
        .fetch_one(&legacy)
        .await
        .unwrap();
    assert_eq!(member.get::<String, _>("email"), "member@example.invalid");
    legacy.close().await;
    let mut admin_url = parsed.clone();
    admin_url.set_path("/postgres");
    let admin = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect(admin_url.as_str())
        .await
        .unwrap();
    let target_name = format!("cbgpu_reverse_{}_test", uuid::Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE DATABASE {target_name}"))
        .execute(&admin)
        .await
        .unwrap();
    let mut target_url = parsed.clone();
    target_url.set_path(&format!("/{target_name}"));
    let target_pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect(target_url.as_str())
        .await
        .unwrap();
    let target = store::Store {
        pool: target_pool.clone(),
        cipher: source.cipher.clone(),
        data: source.data.clone(),
        free_floor: 0,
    };
    target.migrate().await.unwrap();
    sqlx::query("SET ROLE cbgpu_deploy_control")
        .execute(&target_pool)
        .await
        .unwrap();
    let wrong_key = store::Store {
        cipher: cbgpu_deploy_control::crypto::Cipher::new(&[8; 32]).unwrap(),
        ..target.clone()
    };
    assert!(
        reverse::import_bundle(&wrong_key, &new_file, &options)
            .await
            .is_err()
    );
    let bounded = reverse::Options {
        max_rows: 1,
        ..options.clone()
    };
    assert!(
        reverse::import_bundle(&target, &new_file, &bounded)
            .await
            .is_err()
    );
    let empty: i64 = sqlx::query_scalar("SELECT count(*) FROM deploy_store.projects")
        .fetch_one(&target_pool)
        .await
        .unwrap();
    assert_eq!(empty, 0);
    let pristine: serde_json::Value = serde_json::from_str(&envelope).unwrap();
    let tampered = temp.path().join("tampered.sqlite");
    std::fs::copy(&new_file, &tampered).unwrap();
    let tamper_manifest = temp.path().join("tampered.sqlite.manifest.json");
    std::fs::copy(&manifest, &tamper_manifest).unwrap();
    let mut invalid = pristine.clone();
    let mut seal = invalid["sealed_manifest"].as_str().unwrap().to_owned();
    let replacement = if &seal[20..21] == "A" { "B" } else { "A" };
    seal.replace_range(20..21, replacement);
    invalid["sealed_manifest"] = json!(seal);
    std::fs::write(&tamper_manifest, serde_json::to_vec(&invalid).unwrap()).unwrap();
    assert!(
        reverse::import_bundle(&target, &tampered, &options)
            .await
            .is_err()
    );
    let mut decoded = source
        .cipher
        .unseal(pristine["sealed_manifest"].as_str().unwrap())
        .unwrap();
    decoded["schema_sha256"] = json!("0".repeat(64));
    invalid["sealed_manifest"] = json!(source.cipher.seal(&decoded).unwrap());
    std::fs::write(&tamper_manifest, serde_json::to_vec(&invalid).unwrap()).unwrap();
    assert!(
        reverse::import_bundle(&target, &tampered, &options)
            .await
            .is_err()
    );
    std::fs::write(&tamper_manifest, serde_json::to_vec(&pristine).unwrap()).unwrap();
    use std::io::Write;
    std::fs::OpenOptions::new()
        .append(true)
        .open(&tampered)
        .unwrap()
        .write_all(b"tampered-file")
        .unwrap();
    assert!(
        reverse::import_bundle(&target, &tampered, &options)
            .await
            .is_err()
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM deploy_store.projects")
            .fetch_one(&target_pool)
            .await
            .unwrap(),
        0
    );
    let key_path = temp.path().join("fixture-key");
    std::fs::write(&key_path, [7u8; 32]).unwrap();
    let cli_path = temp.path().join("cli-export.sqlite");
    let denied = candidate_command(
        &url,
        temp.path(),
        &key_path,
        &["--export-legacy", cli_path.to_str().unwrap()],
        false,
    )
    .await;
    assert!(!denied.status.success());
    assert!(!cli_path.exists());
    let exported = candidate_command(
        &url,
        temp.path(),
        &key_path,
        &[
            "--export-legacy",
            cli_path.to_str().unwrap(),
            "--confirm-stopped-writers",
        ],
        true,
    )
    .await;
    assert!(
        exported.status.success(),
        "{}",
        String::from_utf8_lossy(&exported.stderr)
    );
    let verified = candidate_command(
        &url,
        temp.path(),
        &key_path,
        &[
            "--verify-reverse-current",
            cli_path.to_str().unwrap(),
            "--confirm-stopped-writers",
        ],
        true,
    )
    .await;
    assert!(
        verified.status.success(),
        "{}",
        String::from_utf8_lossy(&verified.stderr)
    );
    let stale = candidate_command(
        &url,
        temp.path(),
        &key_path,
        &[
            "--verify-reverse-current",
            original.to_str().unwrap(),
            "--confirm-stopped-writers",
        ],
        true,
    )
    .await;
    assert!(!stale.status.success());
    assert!(String::from_utf8_lossy(&stale.stderr).contains("Stale reverse bundle"));
    let inspected = candidate_command(
        &url,
        temp.path(),
        &key_path,
        &["--inspect-reverse", cli_path.to_str().unwrap()],
        false,
    )
    .await;
    assert!(inspected.status.success());
    let safe = String::from_utf8_lossy(&inspected.stdout);
    assert!(!safe.contains("synthetic-private-token") && !safe.contains("member@example.invalid"));
    let cli_bound = candidate_command(
        target_url.as_str(),
        temp.path(),
        &key_path,
        &[
            "--import-reverse-bundle",
            new_file.to_str().unwrap(),
            "--confirm-stopped-writers",
            "--max-rows=1",
        ],
        true,
    )
    .await;
    assert!(!cli_bound.status.success());
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM deploy_store.projects")
            .fetch_one(&target_pool)
            .await
            .unwrap(),
        0
    );
    let restored = reverse::import_bundle(&target, &new_file, &options)
        .await
        .unwrap();
    assert_eq!(restored["full_state_parity"], true);
    assert_eq!(
        importer::target_fingerprint(&target).await.unwrap(),
        current
    );
    assert!(
        reverse::import_bundle(&target, &new_file, &options)
            .await
            .is_err()
    );
    assert_eq!(
        importer::target_fingerprint(&target).await.unwrap(),
        current
    );
    assert_eq!(std::fs::read(&original).unwrap(), old_bytes);
    assert_eq!(
        std::fs::read(&artifact).unwrap(),
        b"private fixture artifact"
    );
    assert!(!std::fs::read_dir(temp.path()).unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".cbgpu-reverse-")
    }));
    target_pool.close().await;
    sqlx::query(&format!("DROP DATABASE {target_name}"))
        .execute(&admin)
        .await
        .unwrap();
    admin.close().await;
    pool.close().await;
}

async fn candidate_command(
    url: &str,
    data: &std::path::Path,
    key: &std::path::Path,
    args: &[&str],
    fenced: bool,
) -> std::process::Output {
    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_cbgpu-deploy-control"));
    command
        .kill_on_drop(true)
        .env_clear()
        .env("CBGPU_DEPLOY_CONTROL_ENABLED", "1")
        .env("CBGPU_DEPLOY_DATABASE_URL", url)
        .env("CBGPU_DEPLOY_DATA", data)
        .env("CBGPU_DEPLOY_KEY_FILE", key)
        .env("CBGPU_DEPLOY_LISTEN", "127.0.0.1:0")
        .env("CBGPU_DEPLOY_LEGACY_LISTEN", "127.0.0.1:0")
        .args(args);
    if fenced {
        command.env("CBGPU_DEPLOY_WRITERS_FENCED", "1");
    }
    tokio::time::timeout(std::time::Duration::from_secs(15), command.output())
        .await
        .unwrap()
        .unwrap()
}
