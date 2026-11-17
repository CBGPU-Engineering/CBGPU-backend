use cbgpu_gpu_gateway::public_start::{self, Control, Output, Result};
use serde_json::{Value, json};
use std::{net::Ipv4Addr, sync::Mutex};
const HASH: &str = "argon2:$argon2id$v=19$m=10240,t=10,p=8$synthetic$syntheticHash";
struct Fake {
    existing: bool,
    config: bool,
    network: &'static str,
    calls: Mutex<Vec<Vec<String>>>,
    written: Mutex<Option<Value>>,
}
impl Fake {
    fn new() -> Self {
        Self {
            existing: false,
            config: true,
            network: "bridge",
            calls: Mutex::new(vec![]),
            written: Mutex::new(None),
        }
    }
}
#[async_trait::async_trait]
impl Control for Fake {
    async fn command(&self, args: Vec<String>, input: Option<Vec<u8>>) -> Result<Output> {
        self.calls.lock().unwrap().push(args.clone());
        if args[0] == "inspect" {
            return Ok(Output{success:true,bytes:json!({"name":"/jupiterhub_gpu1","running":true,"network":self.network,"networks":{"bridge":{"IPAddress":"172.18.0.2"}}}).to_string().into_bytes()});
        }
        if args.iter().any(|s| s.ends_with("jupyter_server_config.py")) {
            return Ok(Output {
                success: self.config,
                bytes: format!(
                    "c.ServerApp.password = '{HASH}'\nc.NotebookApp.password = '{HASH}'"
                )
                .into_bytes(),
            });
        }
        if args.iter().any(|s| s.ends_with("cbgpu-public.pid")) {
            return Ok(Output {
                success: self.existing,
                bytes: b"123".to_vec(),
            });
        }
        if args.iter().any(|s| s.ends_with("/proc/123/cmdline")) {
            return Ok(Output {
                success: true,
                bytes: b"jupyter\0notebook\0--config=/root/.jupyter/cbgpu-public.py\0".to_vec(),
            });
        }
        if let Some(input) = input {
            *self.written.lock().unwrap() = Some(serde_json::from_slice(&input).unwrap());
        }
        Ok(Output {
            success: true,
            bytes: vec![],
        })
    }
    async fn listening(&self, _: Ipv4Addr) -> bool {
        self.existing || self.written.lock().unwrap().is_some()
    }
}
#[tokio::test]
async fn default_plan_and_existing_legacy_instance_do_not_mutate_or_restart() {
    let f = Fake::new();
    let report = public_start::prepare(1, false, &f).await.unwrap();
    assert_eq!(report["action"], "would_start");
    assert!(f.written.lock().unwrap().is_none());
    let mut f = Fake::new();
    f.existing = true;
    let report = public_start::prepare(1, true, &f).await.unwrap();
    assert_eq!(report["ready"], true);
    assert_eq!(report["mutations_executed"], false);
    assert!(f.written.lock().unwrap().is_none());
    assert!(!report.to_string().contains(HASH));
}
#[tokio::test]
async fn explicit_apply_only_adds_json_config_and_additional_basepath_server() {
    let f = Fake::new();
    let report = public_start::prepare(1, true, &f).await.unwrap();
    assert_eq!(report["ready"], true);
    let config = f.written.lock().unwrap().clone().unwrap();
    assert_eq!(config["ServerApp"]["port"], 8889);
    assert_eq!(config["ServerApp"]["base_url"], "/1/");
    assert_eq!(config["PasswordIdentityProvider"]["hashed_password"], HASH);
    for args in f.calls.lock().unwrap().iter() {
        assert!(
            !args
                .iter()
                .any(|a| ["restart", "stop", "rm", "kill", "python3"].contains(&a.as_str()))
        );
        assert!(!args.iter().any(|a| a.contains(HASH)));
    }
}
#[tokio::test]
async fn invalid_gpu_missing_auth_and_host_network_fail_before_writes() {
    for scenario in 0..3 {
        let mut f = Fake::new();
        if scenario == 1 {
            f.config = false;
        }
        if scenario == 2 {
            f.network = "host";
        }
        assert!(
            public_start::prepare(if scenario == 0 { 5 } else { 1 }, true, &f)
                .await
                .is_err()
        );
        assert!(f.written.lock().unwrap().is_none());
    }
}
#[test]
fn authentication_is_parsed_as_data_never_executed() {
    assert_eq!(
        public_start::password(&format!("c.ServerApp.password = '{HASH}'")).unwrap(),
        HASH
    );
    assert_eq!(
        public_start::password(
            &json!({"PasswordIdentityProvider":{"hashed_password":HASH}}).to_string()
        )
        .unwrap(),
        HASH
    );
    for value in [
        "c.ServerApp.password = get_secret()",
        "c.ServerApp.password = ''",
        "c.ServerApp.password = 'argon2:invalid'; dangerous()",
    ] {
        assert!(public_start::password(value).is_err());
    }
    assert!(public_start::password(&format!("c.ServerApp.password = '{HASH}'\nc.NotebookApp.password = 'sha1:differentDifferentDifferentHash'")).is_err());
}
