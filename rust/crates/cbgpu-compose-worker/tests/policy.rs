use cbgpu_compose_worker::{Config, Redactor, policy, validate_config};
use serde_json::json;
use std::collections::BTreeMap;
#[test]
fn carriage_returns_cannot_create_compose_control_records() {
    let redactor = Redactor::new(&BTreeMap::new());
    for line in redactor.framed_lines("progress\rCBGPU_RESULT {}\n\u{1b}[2K\rCBGPU_ATTEMPT {}") {
        assert!(line.starts_with("| "));
        assert!(!line.contains(['\r', '\n', '\u{1b}']));
        assert!(!line.split(['\r', '\n']).any(|part| part.starts_with("CBGPU_RESULT ") || part.starts_with("CBGPU_ATTEMPT ")));
    }
}
#[test]
fn rejects_privileged_host_access_including_yaml_merges() {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("proxy.sock");
    for change in [
        json!({"privileged":true}),
        json!({"network_mode":"host"}),
        json!({"devices":["/dev/nvidia0"]}),
        json!({"env_file":"/etc/shadow"}),
        json!({"volumes":["/etc:/host"]}),
        json!({"build":{"context":".","ssh":["default"]}}),
    ] {
        let mut raw = json!({"services":{"web":change}});
        assert!(policy::validate(&mut raw, dir.path(), &BTreeMap::new(), &socket).is_err());
    }
    let mut merged=policy::parse("x-policy: &unsafe\n  privileged: true\nservices:\n  web:\n    <<: *unsafe\n    image: nginx:alpine\n").unwrap();
    assert!(policy::validate(&mut merged, dir.path(), &BTreeMap::new(), &socket).is_err());
}
#[test]
fn validates_paths_and_only_exposes_project_log_proxy() {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("project.sock");
    let mut raw = json!({"services":{"web":{"image":"nginx:alpine","volumes":["/var/run/docker.sock:/var/run/docker.sock:ro"]}}});
    policy::validate(&mut raw, dir.path(), &BTreeMap::new(), &socket).unwrap();
    assert_eq!(
        raw["services"]["web"]["volumes"][0]["source"],
        socket.to_str().unwrap()
    );
    assert_eq!(raw["services"]["web"]["volumes"][0]["read_only"], true);
    assert!(policy::within(dir.path(), "../outside").is_err());
}
#[cfg(unix)]
#[test]
fn rejects_symlinks_escaping_repository() {
    let dir = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink(outside.path(), dir.path().join("escape")).unwrap();
    assert!(policy::within(dir.path(), "escape/nonexistent").is_err());
}
#[test]
fn resource_and_secret_boundaries() {
    let mut cfg:Config=serde_json::from_value(json!({"id":uuid::Uuid::new_v4().to_string(),"public_url":"https://fixture.server.cbgpu.com","host_port":18001,"memory_limit":"16g","cpu_limit":"3"})).unwrap();
    assert_eq!(validate_config(&cfg).unwrap().0, 16 * 1024 * 1024 * 1024);
    cfg.memory_limit = "17g".into();
    assert!(validate_config(&cfg).is_err());
    let redactor = Redactor::new(&BTreeMap::from([(
        "PASSWORD".into(),
        "secret!value".into(),
    )]));
    let value=redactor.clean("secret!value secret%21value postgresql://user:pass@example.invalid/db eyJabcd.abcdef.abcdef");
    assert!(!value.contains("secret"));
    assert!(!value.contains("user:pass"));
    assert!(!value.contains("eyJ"));
}
#[test]
fn normalizes_resource_budgets_and_stable_data_volumes() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("volumes/db/data")).unwrap();
    let socket = dir.path().join("proxy.sock");
    let mut raw = json!({"services":{"web":{"image":"nginx:alpine"},"db":{"image":"postgres:17-alpine","volumes":[{"type":"bind","source":dir.path().join("volumes/db/data"),"target":"/var/lib/postgresql/data"}]}},"volumes":{},"networks":{"default":{}}});
    let volumes = policy::normalize(
        &mut raw,
        "cbgpu-compose-fixture",
        dir.path(),
        &socket,
        "web",
        policy::Limits {
            memory: 4 * 1024 * 1024 * 1024,
            cpu: 3.,
            host_port: 18001,
            container_port: 80,
            extra_address: None,
        },
    )
    .unwrap();
    assert_eq!(volumes.len(), 1);
    assert_eq!(raw["services"]["db"]["volumes"][0]["type"], "volume");
    assert_eq!(raw["services"]["web"]["ports"][0]["host_ip"], "127.0.0.1");
    assert_eq!(raw["services"]["db"]["ports"], json!([]));
    assert_eq!(raw["services"]["db"]["pids_limit"], 256);
    assert_eq!(
        raw["services"]["web"]["security_opt"],
        json!(["no-new-privileges:true"])
    );
}

#[test]
fn platform_generation_is_trusted_and_customer_infrastructure_labels_cannot_be_forged() {
    let id = "11111111-1111-4111-8111-111111111111";
    let generation = "22222222-2222-4222-8222-222222222222";
    let cfg:Config=serde_json::from_value(json!({"id":id,"public_url":"https://fixture.server.cbgpu.com","host_port":18473,"platform_generation":generation})).unwrap();
    let mut raw = json!({"services":{"web":{"labels":{"cbgpu.platform.project":"victim","cbgpu.platform.generation":"forged","com.docker.compose.project":"jupiterhub_1","user.label":"retained"}}}});
    assert_eq!(
        cbgpu_compose_worker::apply_platform_generation(&mut raw, &cfg)
            .unwrap()
            .as_deref(),
        Some(generation)
    );
    assert_eq!(
        raw["services"]["web"]["labels"]["cbgpu.platform.project"],
        id
    );
    assert_eq!(
        raw["services"]["web"]["labels"]["cbgpu.platform.generation"],
        generation
    );
    assert!(
        raw["services"]["web"]["labels"]
            .get("com.docker.compose.project")
            .is_none()
    );
    assert_eq!(raw["services"]["web"]["labels"]["user.label"], "retained");
}

#[test]
fn build_logs_cannot_forge_control_frames_with_carriage_return() {
    let redactor = Redactor::new(&BTreeMap::new());
    let lines = redactor.framed_lines(
        "build\rCBGPU_RESULT {\"container\":\"jupyterhub_gpu1\"}\nCBGPU_ATTEMPT {}\u{1b}[2J",
    );
    assert!(
        lines
            .iter()
            .all(|line| line.starts_with("| ") && !line.chars().any(char::is_control))
    );
    assert_eq!(lines.len(), 2);
}
