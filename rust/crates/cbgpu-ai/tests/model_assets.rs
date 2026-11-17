use cbgpu_ai::model_assets::{self, Data, Manifest, Model, Remote};
use cbgpu_ai_store::Result;
use serde_json::{Value, json};
use sha1::Sha1;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    sync::atomic::{AtomicUsize, Ordering},
};
const REV: &str = "1111111111111111111111111111111111111111";
fn sha(data: &[u8]) -> String {
    format!("{:x}", Sha256::digest(data))
}
fn blob(data: &[u8]) -> String {
    let mut h = Sha1::new();
    h.update(format!("blob {}\0", data.len()));
    h.update(data);
    format!("{:x}", h.finalize())
}
fn model() -> Model {
    serde_json::from_value(json!({"repo":"example/model","revision":REV,"files":["*.json","model.bin"],"license":"MIT","id":"synthetic"})).unwrap()
}
fn info() -> Value {
    json!({"sha":REV,"siblings":[{"rfilename":"config.json","size":2,"blobId":blob(b"{}")},{"rfilename":"model.bin","size":9,"lfs":{"sha256":sha(b"modeldata"),"size":9}}]})
}
struct Fake {
    bad: bool,
    opens: AtomicUsize,
}
#[async_trait::async_trait]
impl Remote for Fake {
    async fn metadata(&self, repo: &str, revision: &str) -> Result<Value> {
        assert_eq!(repo, "example/model");
        assert_eq!(revision, REV);
        Ok(info())
    }
    async fn open(&self, url: &str) -> Result<Data> {
        self.opens.fetch_add(1, Ordering::SeqCst);
        assert!(url.starts_with(&format!(
            "https://huggingface.co/example/model/resolve/{REV}/"
        )));
        let data = if url.ends_with("config.json") {
            b"{}".to_vec()
        } else if self.bad {
            b"tampered!".to_vec()
        } else {
            b"modeldata".to_vec()
        };
        Ok(Box::pin(futures_util::stream::iter([Ok(data)])))
    }
}
#[tokio::test]
async fn pinned_data_download_is_atomic_hashed_repeatable_and_never_overwrites_changes() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("models");
    let models: Manifest = BTreeMap::from([("chat".into(), model())]);
    let plan = model_assets::offline_plan(&models, &root);
    assert_eq!(plan["downloads_performed"], false);
    assert!(!root.exists());
    let remote = Fake {
        bad: false,
        opens: AtomicUsize::new(0),
    };
    let result = model_assets::prepare(&models, &root, &remote)
        .await
        .unwrap();
    assert_eq!(remote.opens.load(Ordering::SeqCst), 2);
    assert_eq!(
        std::fs::read(root.join("chat/model.bin")).unwrap(),
        b"modeldata"
    );
    assert!(result["models"].as_array().unwrap().iter().all(|row| {
        row["asset"]["sha256"]
            .as_str()
            .is_some_and(|v| v.len() == 64)
    }));
    model_assets::prepare(&models, &root, &remote)
        .await
        .unwrap();
    assert_eq!(remote.opens.load(Ordering::SeqCst), 2);
    std::fs::write(root.join("chat/model.bin"), b"tampered!").unwrap();
    assert!(
        model_assets::prepare(&models, &root, &remote)
            .await
            .is_err()
    );
    assert_eq!(
        std::fs::read(root.join("chat/model.bin")).unwrap(),
        b"tampered!"
    );
}
#[tokio::test]
async fn failed_hash_or_symlink_never_publishes_a_partial_asset() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("models");
    let models: Manifest = BTreeMap::from([("chat".into(), model())]);
    let remote = Fake {
        bad: true,
        opens: AtomicUsize::new(0),
    };
    assert!(
        model_assets::prepare(&models, &root, &remote)
            .await
            .is_err()
    );
    assert!(!root.join("chat/model.bin").exists());
    assert!(std::fs::read_dir(root.join("chat")).unwrap().all(|f| {
        !f.unwrap()
            .file_name()
            .to_string_lossy()
            .ends_with(".partial")
    }));
    let outside = temp.path().join("outside");
    std::fs::create_dir(&outside).unwrap();
    std::fs::remove_dir_all(root.join("chat")).unwrap();
    std::os::unix::fs::symlink(&outside, root.join("chat")).unwrap();
    assert!(
        model_assets::prepare(&models, &root, &remote)
            .await
            .is_err()
    );
    assert_eq!(std::fs::read_dir(outside).unwrap().count(), 0);
}
#[test]
fn executable_paths_bad_revisions_and_missing_pins_are_refused() {
    let mut spec = model();
    let mut metadata = info();
    metadata["sha"] = "moving-branch".into();
    assert!(model_assets::assets(&spec, &metadata).is_err());
    metadata = info();
    metadata["siblings"][1]["rfilename"] = "../model.bin".into();
    assert!(model_assets::assets(&spec, &metadata).is_err());
    spec.files = vec!["*".into()];
    metadata = info();
    metadata["siblings"][1]["rfilename"] = "execute.py".into();
    assert!(model_assets::assets(&spec, &metadata).is_err());
    spec = model();
    metadata = info();
    metadata["siblings"].as_array_mut().unwrap().remove(1);
    assert!(model_assets::assets(&spec, &metadata).is_err());
}
#[test]
fn only_approved_https_data_hosts_are_eligible_for_dns_pinned_requests() {
    for good in [
        "https://huggingface.co/example/model",
        "https://cas-bridge.xethub.hf.co/data",
    ] {
        assert!(model_assets::allowed_hub_url(
            &url::Url::parse(good).unwrap()
        ));
    }
    for bad in [
        "http://huggingface.co/x",
        "https://huggingface.co.evil.test/x",
        "https://user:pass@huggingface.co/x",
        "https://127.0.0.1/x",
        "https://huggingface.co:8080/x",
    ] {
        assert!(!model_assets::allowed_hub_url(
            &url::Url::parse(bad).unwrap()
        ));
    }
}
#[test]
fn shipped_manifest_is_data_only_and_native_whisper_pins_match_runtime() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../..")
        .join("ai/models.json");
    let models = model_assets::manifest(&std::fs::read(path).unwrap()).unwrap();
    assert_eq!(
        models["transcribe"].local_dir.as_deref(),
        Some("transcribe-native")
    );
    assert_eq!(
        models["transcribe"].sha256["ggml-large-v3.bin"],
        "64d182b440b98d5203c4f9bd541544d84c605196c4f7b845dfa11fb23594d1e2"
    );
    assert_eq!(
        models["vad"].local_dir.as_deref(),
        Some("transcribe-native")
    );
}
