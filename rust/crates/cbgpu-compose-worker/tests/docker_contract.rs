use cbgpu_compose_worker::{Config, deploy};
use serde_json::{Value, json};
use std::{path::PathBuf, process::Command};
fn docker(args: &[&str]) -> String {
    let out = Command::new("docker").args(args).output().unwrap();
    assert!(out.status.success(), "fixture Docker command failed");
    String::from_utf8_lossy(&out.stdout).trim().into()
}
struct Fixture {
    _temp: tempfile::TempDir,
    base: PathBuf,
    source: PathBuf,
    id: String,
    namespace: String,
    config: Config,
}
impl Fixture {
    fn new(doc: Value) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let base = temp.path().join("private/compose-deployments");
        let source = temp.path().join("fixture");
        std::fs::create_dir_all(&base).unwrap();
        std::fs::create_dir_all(&source).unwrap();
        std::fs::write(
            source.join("compose.yml"),
            serde_yaml_ng::to_string(&doc).unwrap(),
        )
        .unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        let namespace = format!("cbgpu-compose-{id}");
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let config=serde_json::from_value(json!({"id":id,"public_url":"https://fixture.server.cbgpu.com","host_port":port,"container_port":80,"memory_limit":"512m","cpu_limit":"0.5","env_vars":{"CBGPU_GATEWAY_SERVICE":"web","CHECK_VALUE":"fixture$literal'quote","REVISION":"1"}})).unwrap();
        Self {
            _temp: temp,
            base,
            source,
            id,
            namespace,
            config,
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let output = Command::new("docker")
            .args([
                "ps",
                "-a",
                "--filter",
                &format!("label=com.docker.compose.project={}", self.namespace),
                "--format",
                "{{.ID}}",
            ])
            .output();
        if let Ok(out) = output {
            for id in String::from_utf8_lossy(&out.stdout).split_whitespace() {
                let _ = Command::new("docker").args(["rm", "-f", id]).output();
            }
        }
        for (kind, field) in [("volume", "{{.Name}}"), ("network", "{{.Name}}")] {
            if let Ok(out) = Command::new("docker")
                .args([
                    kind,
                    "ls",
                    "--filter",
                    &format!("label=com.docker.compose.project={}", self.namespace),
                    "--format",
                    field,
                ])
                .output()
            {
                for name in String::from_utf8_lossy(&out.stdout)
                    .lines()
                    .filter(|n| n.starts_with(&format!("{}-", self.namespace)))
                {
                    let _ = Command::new("docker").args([kind, "rm", name]).output();
                }
            }
        }
        let _ = Command::new("docker")
            .args([
                "image",
                "rm",
                "-f",
                &format!("{}-web:build", self.namespace),
            ])
            .output();
        let _ = Command::new("docker")
            .args([
                "buildx",
                "rm",
                &format!("cbgpu-compose-build-{}", &self.id[..12]),
            ])
            .env(
                "DOCKER_CONFIG",
                self.base.parent().unwrap().join("compose-docker-client"),
            )
            .output();
    }
}
fn enabled() {
    assert_eq!(
        std::env::var("CBGPU_COMPOSE_RUN_DOCKER_TEST").as_deref(),
        Ok("1"),
        "Explicitly enable temporary Docker contracts"
    );
}
#[tokio::test]
#[ignore = "uses only temporary, namespace-scoped Docker fixtures"]
async fn deploy_redeploy_preserves_volumes_literals_and_resource_limits() {
    enabled();
    let mut f = Fixture::new(
        json!({"services":{"web":{"image":"nginx:alpine","build":{"context":"./web"},"environment":{"CHECK_VALUE":"${CHECK_VALUE}"}},"db":{"image":"nginx:alpine","entrypoint":["/bin/sh","-c"],"command":["n=$$(cat /data/counter 2>/dev/null || echo 0); n=$$((n+1)); echo $$n > /data/counter; exec tail -f /dev/null"],"environment":{"REVISION":"${REVISION}"},"volumes":["./volumes/db/data:/data"]}}}),
    );
    std::fs::create_dir_all(f.source.join("volumes/db/data")).unwrap();
    std::fs::create_dir_all(f.source.join("web")).unwrap();
    std::fs::write(f.source.join("web/Dockerfile"), "FROM nginx:alpine\n").unwrap();
    let mut prior = None;
    for revision in ["1", "2"] {
        let generation = uuid::Uuid::new_v4().to_string();
        f.config.platform_generation = Some(generation.clone());
        f.config.env_vars.insert("REVISION".into(), revision.into());
        let result = deploy(&f.config, &f.base, Some(&f.source)).await.unwrap();
        assert_eq!(
            docker(&[
                "exec",
                &format!("{}-db", f.namespace),
                "cat",
                "/data/counter"
            ]),
            revision
        );
        assert_eq!(
            docker(&[
                "exec",
                &format!("{}-web", f.namespace),
                "printenv",
                "CHECK_VALUE"
            ]),
            "fixture$literal'quote"
        );
        let managed: Value =
            serde_json::from_str(&std::fs::read_to_string(&result.file).unwrap()).unwrap();
        assert_eq!(
            managed["services"]["web"]["ports"][0]["host_ip"],
            "127.0.0.1"
        );
        assert_eq!(managed["services"]["db"]["pids_limit"], 256);
        assert_eq!(
            result.platform_generation.as_deref(),
            Some(generation.as_str())
        );
        let containers: Value =
            serde_json::from_str(&docker(&["inspect", &format!("{}-web", f.namespace)])).unwrap();
        assert_eq!(
            containers[0]["Config"]["Labels"]["cbgpu.platform.generation"],
            generation
        );
        assert_eq!(
            containers[0]["Config"]["Labels"]["cbgpu.platform.project"],
            f.id
        );

        if let Some(volumes) = prior {
            assert_eq!(result.volumes, volumes);
        }
        assert!(
            cbgpu_compose_worker::measure(&result, &f.base)
                .await
                .unwrap()
                > 0
        );
        prior = Some(result.volumes);
    }
}
#[tokio::test]
#[ignore = "uses only temporary, namespace-scoped Docker fixtures"]
async fn unhealthy_services_retain_data_and_redacted_health_evidence() {
    enabled();
    let secret = "fixture-diagnostics-secret-12345";
    let mut f = Fixture::new(
        json!({"services":{"web":{"image":"nginx:alpine","entrypoint":["/bin/sh","-c"],"command":["echo schema-cache-unavailable: $$DB_PASSWORD; exec tail -f /dev/null"],"environment":{"DB_PASSWORD":secret},"healthcheck":{"test":["CMD-SHELL","echo database-connection-refused: $$DB_PASSWORD; exit 1"],"interval":"1s","timeout":"1s","retries":1}},"db":{"image":"nginx:alpine","entrypoint":["/bin/sh","-c"],"command":["echo retained > /data/preserved; exec tail -f /dev/null"],"volumes":["db-data:/data"]}},"volumes":{"db-data":{}}}),
    );
    f.config
        .env_vars
        .insert("DB_PASSWORD".into(), secret.into());
    assert!(deploy(&f.config, &f.base, Some(&f.source)).await.is_err());
    let project = f.base.join(&f.id);
    let release = std::fs::read_dir(project)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let text = std::fs::read_to_string(release.join("failure-diagnostics.json")).unwrap();
    assert!(!text.contains(secret));
    let value: Value = serde_json::from_str(&text).unwrap();
    assert!(
        value["failed_services"]
            .as_array()
            .unwrap()
            .contains(&json!("web"))
    );
    assert!(
        value["container_logs"]
            .as_str()
            .unwrap()
            .contains("schema-cache-unavailable")
    );
    assert!(
        value["container_logs"]
            .as_str()
            .unwrap()
            .contains("[REDACTED]")
    );
    let service = value["services"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["service"] == "web")
        .unwrap();
    assert_eq!(service["health"], "unhealthy");
    assert!(
        service["health_output"]
            .as_str()
            .unwrap()
            .contains("database-connection-refused")
    );
    use sqlx::{
        Row,
        sqlite::{SqliteConnectOptions, SqlitePoolOptions},
    };
    let state_dir = f.base.parent().unwrap().join("frontend-deployments");
    std::fs::create_dir_all(&state_dir).unwrap();
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(
            SqliteConnectOptions::new()
                .filename(state_dir.join("deploy.sqlite"))
                .create_if_missing(true),
        )
        .await
        .unwrap();
    sqlx::query("CREATE TABLE compose_states(project_id TEXT PRIMARY KEY,status TEXT DEFAULT 'error',attempt_result TEXT,result TEXT,logs TEXT,diagnostics TEXT,error TEXT,updated_at TEXT)").execute(&pool).await.unwrap();
    let managed: Value = serde_json::from_str(
        &std::fs::read_to_string(release.join("compose-managed.json")).unwrap(),
    )
    .unwrap();
    let attempt = cbgpu_compose_worker::Attempt {
        platform_generation: None,
        namespace: f.namespace.clone(),
        file: release.join("compose-managed.json"),
        release: release.clone(),
        gateway: "web".into(),
        volumes: managed["volumes"]
            .as_object()
            .unwrap()
            .values()
            .map(|v| v["name"].as_str().unwrap().to_owned())
            .collect(),
        services: vec!["db".into(), "web".into()],
        socket: f
            .base
            .parent()
            .unwrap()
            .join("compose-sockets")
            .join(format!("{}.sock", f.id)),
        storage_gb: 40,
    };
    sqlx::query("INSERT INTO compose_states(project_id,attempt_result,logs) VALUES(?,?,?)")
        .bind(&f.id)
        .bind(serde_json::to_string(&attempt).unwrap())
        .bind("previous logs")
        .execute(&pool)
        .await
        .unwrap();
    cbgpu_compose_worker::capture_existing(&f.id, &f.base)
        .await
        .unwrap();
    let saved = sqlx::query("SELECT diagnostics,logs,error FROM compose_states WHERE project_id=?")
        .bind(&f.id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(saved.get::<String, _>("error").contains("web"));
    assert!(!saved.get::<String, _>("diagnostics").contains(secret));
    assert!(
        saved
            .get::<String, _>("logs")
            .contains("schema-cache-unavailable")
    );
    pool.close().await;
    assert_eq!(
        docker(&[
            "inspect",
            "--format",
            "{{.State.Running}}",
            &format!("{}-web", f.namespace)
        ]),
        "true"
    );
    assert_eq!(
        docker(&[
            "exec",
            &format!("{}-db", f.namespace),
            "cat",
            "/data/preserved"
        ]),
        "retained"
    );
}
