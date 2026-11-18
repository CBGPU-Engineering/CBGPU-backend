//! Explicit notebook mode only. Inventory mode never constructs Docker or relay clients.
use super::{
    notebook_client::{Client, Http, Policy},
    notebook_executor::{Executor, LocalDocker},
    notebook_relay,
};
use ed25519_dalek::SigningKey;
use std::{path::Path, sync::Arc};
pub async fn run(
    identity: &Path,
    policy_path: &Path,
    origin: &str,
    node: &str,
    version: i64,
    key: SigningKey,
) -> Result<(), &'static str> {
    let policy = Policy::read(policy_path)?;
    let server_key = policy.server_public_key.clone();
    let image = policy.image_digest.clone();
    let root = identity
        .parent()
        .ok_or("PRIVATE_FILE_INVALID")?
        .join("notebook-runtime-v3")
        .join(node);
    let executor = Arc::new(Executor::open(
        Arc::new(LocalDocker::open()?),
        &root,
        node,
        &image,
    )?);
    let _executor_lock = super::installer::executor_lock(&root)?;
    let journal = identity.with_extension("notebook-control-v3.json");
    let mut client = Client::open(
        Http::open(origin)?,
        key.clone(),
        policy,
        &journal,
        node,
        version,
        executor.clone(),
    )?;
    let watchdog = executor.clone();
    let watch = tokio::spawn(async move {
        loop {
            let now = super::notebook_client::time().unwrap_or(i64::MAX);
            if watchdog.enforce_deadlines(now).await.is_err() {
                eprintln!(
                    "Notebook deadline enforcement needs operator inspection; no unknown container was stopped."
                );
            }
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        }
    });
    let origin = origin.to_owned();
    let node = node.to_owned();
    let relay_executor = executor.clone();
    let relay = tokio::spawn(async move {
        let mut failures = 0;
        loop {
            let result = notebook_relay::connect(
                &origin,
                &node,
                version,
                &key,
                &server_key,
                relay_executor.clone(),
            )
            .await;
            if result == Err("NODE_REVOKED") {
                break;
            }
            let delay = super::installer::retry_seconds(failures);
            failures = failures.saturating_add(1);
            tokio::time::sleep(std::time::Duration::from_secs(delay)).await;
        }
    });
    let result:Result<(),&'static str>=async {
        let mut interval=tokio::time::interval(std::time::Duration::from_secs(10));
        loop {
            interval.tick().await;
            let hardware=super::collect().map_err(|_|"NVIDIA_UNAVAILABLE")?.gpus.into_iter().filter_map(|g|g.hardware_uuid).collect();
            if client.capabilities(hardware).await.is_err(){
                eprintln!("Notebook capability preflight unavailable; execution remains unadvertised and local fences continue.");
                continue;
            }
            if client.tick().await.is_err(){
                eprintln!("Notebook control unavailable; signed local deadlines remain enforced.");
            }
        }
    }.await;
    // Do not cancel the watchdog while any unconfirmed workload remains: revoke only our journal-owned jobs.
    let stopped = executor.stop_all().await;
    watch.abort();
    relay.abort();
    result.and(stopped)
}
