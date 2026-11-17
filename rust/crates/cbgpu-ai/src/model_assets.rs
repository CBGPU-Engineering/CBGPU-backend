//! Pinned, non-executable public model data. Offline plan by default; no token/cache auth.
use cbgpu_ai_store::{Result, fail};
use futures_util::{Stream, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha1::Sha1;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    io::Read,
    path::{Path, PathBuf},
    pin::Pin,
    time::Duration,
};
use tokio::io::AsyncWriteExt;
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Model {
    pub repo: String,
    pub revision: String,
    pub files: Vec<String>,
    pub license: String,
    pub id: String,
    #[serde(default)]
    pub local_dir: Option<String>,
    #[serde(default)]
    pub sha256: BTreeMap<String, String>,
}
pub type Manifest = BTreeMap<String, Model>;
pub type Data = Pin<Box<dyn Stream<Item = Result<Vec<u8>>> + Send>>;
#[async_trait::async_trait]
pub trait Remote: Send + Sync {
    async fn metadata(&self, repo: &str, revision: &str) -> Result<Value>;
    async fn open(&self, url: &str) -> Result<Data>;
}
pub struct PublicHub;
pub fn allowed_hub_url(url: &url::Url) -> bool {
    url.scheme() == "https"
        && url.username().is_empty()
        && url.password().is_none()
        && url.fragment().is_none()
        && url.port_or_known_default() == Some(443)
        && url.host_str().is_some_and(|host| {
            host == "huggingface.co"
                || host.ends_with(".huggingface.co")
                || host.ends_with(".hf.co")
        })
}
impl PublicHub {
    async fn response(&self, input: &str) -> Result<reqwest::Response> {
        let mut url = url::Url::parse(input).map_err(|_| fail("Invalid model data URL", 400))?;
        for _ in 0..6 {
            if !allowed_hub_url(&url) {
                return Err(fail("Model redirect left approved public data hosts", 400));
            }
            let (host, address) = crate::web::public_target(&url).await?;
            let client = reqwest::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .resolve(&host, address)
                .connect_timeout(Duration::from_secs(15))
                .timeout(Duration::from_secs(7200))
                .build()
                .map_err(|_| fail("Model downloader unavailable", 503))?;
            let response = client
                .get(url.clone())
                .header("user-agent", "CBGPU-native-model-assets/1")
                .send()
                .await
                .map_err(|_| fail("Public model data unavailable", 502))?;
            if response.status().is_redirection() {
                let next = response
                    .headers()
                    .get("location")
                    .and_then(|v| v.to_str().ok())
                    .ok_or_else(|| fail("Invalid model redirect", 502))?;
                url = url
                    .join(next)
                    .map_err(|_| fail("Invalid model redirect", 502))?;
                continue;
            }
            if !response.status().is_success() {
                return Err(fail("Public model data unavailable", 502));
            }
            return Ok(response);
        }
        Err(fail("Too many model redirects", 502))
    }
}
#[async_trait::async_trait]
impl Remote for PublicHub {
    async fn metadata(&self, repo: &str, revision: &str) -> Result<Value> {
        let response = self
            .response(&format!(
                "https://huggingface.co/api/models/{repo}/revision/{revision}?blobs=true"
            ))
            .await?;
        let mut stream = response.bytes_stream();
        let mut data = Vec::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|_| fail("Model metadata interrupted", 502))?;
            if data.len() + chunk.len() > 2 * 1024 * 1024 {
                return Err(fail("Model metadata exceeds bound", 502));
            }
            data.extend_from_slice(&chunk);
        }
        serde_json::from_slice(&data).map_err(|_| fail("Invalid model metadata", 502))
    }
    async fn open(&self, url: &str) -> Result<Data> {
        let response = self.response(url).await?;
        Ok(Box::pin(response.bytes_stream().map(|chunk| {
            chunk
                .map(|v| v.to_vec())
                .map_err(|_| fail("Model data interrupted", 502))
        })))
    }
}
fn relative(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 240
        && !value.starts_with('/')
        && value.split('/').all(|s| {
            !s.is_empty()
                && s != "."
                && s != ".."
                && !s.starts_with('.')
                && s.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
        })
}
fn digest_valid(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit())
}
pub fn manifest(bytes: &[u8]) -> Result<Manifest> {
    if bytes.len() > 1 << 20 {
        return Err(fail("Model manifest exceeds bound", 400));
    }
    let result: Manifest =
        serde_json::from_slice(bytes).map_err(|_| fail("Invalid model manifest", 400))?;
    if result.is_empty() || result.len() > 32 {
        return Err(fail("Invalid model manifest", 400));
    }
    for (name, m) in &result {
        if !relative(name)
            || name.contains('/')
            || m.repo.split('/').count() != 2
            || !relative(&m.repo)
            || m.revision.len() != 40
            || !m.revision.bytes().all(|b| b.is_ascii_hexdigit())
            || m.files.is_empty()
            || m.files.len() > 32
            || m.license.is_empty()
            || m.id.is_empty()
            || m.local_dir
                .as_ref()
                .is_some_and(|p| !relative(p) || p.contains('/'))
            || m.sha256
                .iter()
                .any(|(name, hash)| !relative(name) || !digest_valid(hash))
        {
            return Err(fail(
                "Model paths, revisions and hashes must be pinned",
                400,
            ));
        }
        for pattern in &m.files {
            if pattern.len() > 240
                || pattern.contains("..")
                || pattern.starts_with('/')
                || !pattern
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"._-/*?".contains(&b))
            {
                return Err(fail("Unsafe model file pattern", 400));
            }
        }
    }
    Ok(result)
}
pub fn offline_plan(models: &Manifest, root: &Path) -> Value {
    json!({"mode":"offline-plan","downloads_performed":false,"gpu_jobs_started":false,"models":models.iter().map(|(key,m)|json!({"name":key,"repo":m.repo,"revision":m.revision,"license":m.license,"patterns":m.files,"directory":root.join(m.local_dir.as_ref().unwrap_or(key))})).collect::<Vec<_>>()})
}
#[derive(Clone)]
pub struct Asset {
    pub name: String,
    pub size: u64,
    pub sha256: Option<String>,
    pub git_blob: Option<String>,
}
pub fn assets(model: &Model, info: &Value) -> Result<Vec<Asset>> {
    if info["sha"] != model.revision {
        return Err(fail("Model provider revision does not match pin", 502));
    }
    let siblings = info["siblings"]
        .as_array()
        .filter(|v| v.len() <= 10000)
        .ok_or_else(|| fail("Invalid model file metadata", 502))?;
    let patterns = model
        .files
        .iter()
        .map(|p| {
            regex::Regex::new(&format!(
                "^{}$",
                regex::escape(p).replace("\\*", ".*").replace("\\?", ".")
            ))
            .map_err(|_| fail("Invalid model pattern", 400))
        })
        .collect::<Result<Vec<_>>>()?;
    let mut result = BTreeMap::new();
    for file in siblings {
        let Some(name) = file["rfilename"].as_str() else {
            continue;
        };
        if !patterns.iter().any(|p| p.is_match(name)) {
            continue;
        }
        if !relative(name)
            || !name.rsplit('.').next().is_some_and(|v| {
                ["json", "txt", "model", "safetensors", "gguf", "bin"].contains(&v)
            })
        {
            return Err(fail("Executable or unsafe model artifact refused", 400));
        }
        let size = file["size"]
            .as_u64()
            .or_else(|| file["lfs"]["size"].as_u64())
            .filter(|s| *s > 0 && *s <= 64 * 1024 * 1024 * 1024)
            .ok_or_else(|| fail("Model artifact size missing or exceeds bound", 502))?;
        let hash = model
            .sha256
            .get(name)
            .cloned()
            .or_else(|| file["lfs"]["sha256"].as_str().map(str::to_owned));
        if hash.as_ref().is_some_and(|v| !digest_valid(v)) {
            return Err(fail("Invalid model artifact hash", 502));
        }
        if let (Some(expected), Some(advertised)) =
            (model.sha256.get(name), file["lfs"]["sha256"].as_str())
            && expected != advertised
        {
            return Err(fail("Model artifact hash differs from reviewed pin", 502));
        }
        if matches!(
            name.rsplit('.').next(),
            Some("gguf" | "safetensors" | "bin")
        ) && hash.is_none()
        {
            return Err(fail("Binary model artifact requires SHA256", 502));
        }
        let git_blob = if hash.is_none() {
            Some(
                file["blobId"]
                    .as_str()
                    .filter(|v| v.len() == 40 && v.bytes().all(|b| b.is_ascii_hexdigit()))
                    .ok_or_else(|| fail("Pinned Git blob identity missing", 502))?
                    .to_owned(),
            )
        } else {
            None
        };
        result.insert(
            name.to_owned(),
            Asset {
                name: name.to_owned(),
                size,
                sha256: hash,
                git_blob,
            },
        );
    }
    if model
        .files
        .iter()
        .filter(|p| !p.contains(['*', '?']))
        .any(|name| !result.contains_key(name))
    {
        return Err(fail(
            "Pinned model file missing from provider metadata",
            502,
        ));
    }
    if result.is_empty() {
        return Err(fail("Pinned model patterns matched no data files", 502));
    }
    Ok(result.into_values().collect())
}
fn safe_parent(root: &Path, relative: &str) -> Result<PathBuf> {
    let mut parent = root.to_owned();
    let components = Path::new(relative).components().collect::<Vec<_>>();
    for item in &components[..components.len() - 1] {
        parent.push(item.as_os_str());
        match std::fs::symlink_metadata(&parent) {
            Ok(meta) if meta.is_dir() && !meta.file_type().is_symlink() => {}
            Ok(_) => return Err(fail("Model artifact parent is not a real directory", 400)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                std::fs::create_dir(&parent)?;
            }
            Err(error) => return Err(error.into()),
        }
    }
    Ok(parent)
}
fn existing(path: &Path, asset: &Asset) -> Result<Option<String>> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) => {
            if !meta.is_file() || meta.file_type().is_symlink() || meta.len() != asset.size {
                return Err(fail(
                    "Existing model asset differs; refusing overwrite",
                    409,
                ));
            }
            let mut file = std::fs::File::open(path)?;
            let mut sha = Sha256::new();
            let mut git = Sha1::new();
            git.update(format!("blob {}\0", asset.size).as_bytes());
            let mut bytes = vec![0u8; 1024 * 1024];
            loop {
                let n = file.read(&mut bytes)?;
                if n == 0 {
                    break;
                }
                sha.update(&bytes[..n]);
                git.update(&bytes[..n]);
            }
            let sha = format!("{:x}", sha.finalize());
            let git = format!("{:x}", git.finalize());
            if asset
                .sha256
                .as_ref()
                .is_some_and(|expected| *expected != sha)
                || asset
                    .git_blob
                    .as_ref()
                    .is_some_and(|expected| *expected != git)
            {
                return Err(fail(
                    "Existing model asset differs; refusing overwrite",
                    409,
                ));
            }
            Ok(Some(sha))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}
struct Partial(PathBuf);
impl Drop for Partial {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}
pub async fn download_one(
    root: &Path,
    model: &Model,
    asset: &Asset,
    remote: &dyn Remote,
) -> Result<Value> {
    if !relative(&asset.name) {
        return Err(fail("Unsafe model artifact path", 400));
    }
    let parent = safe_parent(root, &asset.name)?;
    let destination = root.join(&asset.name);
    if let Some(sha256) = existing(&destination, asset)? {
        return Ok(json!({"file":asset.name,"state":"verified-existing","sha256":sha256}));
    }
    let partial = Partial(parent.join(format!(".cbgpu-model-{}.partial", uuid::Uuid::new_v4())));
    let mut options = tokio::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options.open(&partial.0).await?;
    let mut stream = remote
        .open(&format!(
            "https://huggingface.co/{}/resolve/{}/{}",
            model.repo, model.revision, asset.name
        ))
        .await?;
    let mut bytes = 0u64;
    let mut hash = Sha256::new();
    let mut git = Sha1::new();
    git.update(format!("blob {}\0", asset.size).as_bytes());
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        bytes = bytes
            .checked_add(chunk.len() as u64)
            .ok_or_else(|| fail("Model artifact exceeds bound", 502))?;
        if bytes > asset.size {
            return Err(fail("Model artifact exceeds pinned size", 502));
        }
        hash.update(&chunk);
        git.update(&chunk);
        file.write_all(&chunk).await?;
    }
    if bytes != asset.size {
        return Err(fail("Model artifact was truncated", 502));
    }
    let hash = format!("{:x}", hash.finalize());
    if asset
        .sha256
        .as_ref()
        .is_some_and(|expected| *expected != hash)
    {
        return Err(fail("Model artifact failed SHA256 verification", 502));
    }
    if asset
        .git_blob
        .as_ref()
        .is_some_and(|expected| *expected != format!("{:x}", git.finalize()))
    {
        return Err(fail(
            "Model artifact failed pinned Git blob verification",
            502,
        ));
    }
    file.sync_all().await?;
    drop(file);
    std::fs::hard_link(&partial.0, &destination)
        .map_err(|_| fail("Model target changed; refusing overwrite", 409))?;
    Ok(json!({"file":asset.name,"bytes":bytes,"sha256":hash,"state":"downloaded"}))
}
pub async fn prepare(models: &Manifest, root: &Path, remote: &dyn Remote) -> Result<Value> {
    let _validated = manifest(&serde_json::to_vec(models)?)?;
    if !root.is_absolute() || root.parent().is_none() {
        return Err(fail("Model root must be absolute", 400));
    }
    std::fs::create_dir_all(root)?;
    let root = root.canonicalize()?;
    #[cfg(unix)]
    let _lock = {
        use std::os::{fd::AsRawFd, unix::fs::OpenOptionsExt};
        let lock = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(root.join(".cbgpu-prepare.lock"))?;
        if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            return Err(fail("Another native model preparer holds this root", 409));
        }
        lock
    };
    let mut results = Vec::new();
    let mut total = 0u64;
    for (name, model) in models {
        let info = remote.metadata(&model.repo, &model.revision).await?;
        let files = assets(model, &info)?;
        let directory = model.local_dir.as_ref().unwrap_or(name);
        let destination = root.join(directory);
        match std::fs::symlink_metadata(&destination) {
            Ok(meta) if !meta.is_dir() || meta.file_type().is_symlink() => {
                return Err(fail("Model directory must be real", 400));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                std::fs::create_dir(&destination)?
            }
            Err(error) => return Err(error.into()),
        }
        for asset in files {
            total = total
                .checked_add(asset.size)
                .filter(|v| *v <= 128 * 1024 * 1024 * 1024)
                .ok_or_else(|| fail("Manifest exceeds total 128GiB budget", 400))?;
            let result = download_one(&destination, model, &asset, remote).await?;
            results.push(json!({"model":name,"repo":model.repo,"revision":model.revision,"license":model.license,"asset":result}));
        }
    }
    let receipt = json!({"mode":"explicit-model-data-download","models":results,"gpu_jobs_started":false,"service_changes":false});
    let path = root.join(format!(
        ".cbgpu-native-assets-{}.json",
        uuid::Uuid::new_v4()
    ));
    let mut options = std::fs::OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    use std::io::Write;
    file.write_all(&serde_json::to_vec_pretty(&receipt)?)?;
    file.sync_all()?;
    Ok(receipt)
}
