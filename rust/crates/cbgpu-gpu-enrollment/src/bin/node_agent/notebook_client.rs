//! Explicit protocol-v3 control. Persist-before-send, signed response binding and independent executor fences.
use super::notebook_executor::{Docker, Executor};
use cbgpu_gpu_enrollment::{
    hex,
    notebooks::protocol::{
        CommandLease, Envelope, NodeRequest, NotebookResult, Payload, SignedEnvelope,
        SignedNodeRequest, sign_node_request, verify_bound_envelope,
    },
    unhex, valid_hardware_uuid,
};
use ed25519_dalek::{SigningKey, VerifyingKey};
use serde::{Deserialize, Serialize};
use std::{
    future::Future,
    path::{Path, PathBuf},
    sync::Arc,
};
pub type Error = &'static str;
pub type Result<T> = std::result::Result<T, Error>;
pub trait Transport: Send + Sync {
    fn send(
        &self,
        path: &str,
        request: &SignedNodeRequest,
    ) -> impl Future<Output = Result<SignedEnvelope>> + Send;
}
pub struct Http {
    origin: String,
    client: reqwest::Client,
}
impl Http {
    pub fn open(origin: &str) -> Result<Self> {
        let u = url::Url::parse(origin).map_err(|_| "INVALID_REGISTRY")?;
        if u.scheme() != "https"
            || u.host_str().is_none()
            || u.port_or_known_default() != Some(443)
            || u.path() != "/"
            || u.query().is_some()
            || u.fragment().is_some()
            || !u.username().is_empty()
            || u.password().is_some()
        {
            return Err("INVALID_REGISTRY");
        }
        Ok(Self {
            origin: origin.trim_end_matches('/').into(),
            client: reqwest::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(std::time::Duration::from_secs(15))
                .build()
                .map_err(|_| "CONTROL_UNAVAILABLE")?,
        })
    }
}
impl Transport for Http {
    async fn send(&self, path: &str, request: &SignedNodeRequest) -> Result<SignedEnvelope> {
        let mut r = self
            .client
            .post(format!("{}{path}", self.origin))
            .json(request)
            .send()
            .await
            .map_err(|_| "CONTROL_UNAVAILABLE")?;
        if !r.status().is_success() {
            return Err("CONTROL_REJECTED");
        }
        let mut bytes = Vec::new();
        while let Some(c) = r.chunk().await.map_err(|_| "CONTROL_UNAVAILABLE")? {
            if bytes.len() + c.len() > 65536 {
                return Err("CONTROL_RESPONSE_TOO_LARGE");
            }
            bytes.extend(c)
        }
        serde_json::from_slice(&bytes).map_err(|_| "INVALID_CONTROL_RESPONSE")
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Policy {
    pub protocol: u16,
    pub enabled: bool,
    pub server_public_key: String,
    pub image_digest: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Pending {
    lease: CommandLease,
    result: Option<NotebookResult>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Journal {
    protocol: u16,
    node_id: String,
    version: i64,
    server_key: String,
    image: String,
    sequence: i64,
    pending: Option<Pending>,
    active: Vec<CommandLease>,
    seen: Vec<(String, i64)>,
}
pub struct Client<T: Transport, D: Docker> {
    transport: T,
    key: SigningKey,
    policy: Policy,
    path: PathBuf,
    state: Journal,
    executor: Arc<Executor<D>>,
}
pub fn time() -> Result<i64> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| "CLOCK_UNAVAILABLE")?
        .as_secs()
        .try_into()
        .map_err(|_| "CLOCK_UNAVAILABLE")
}
fn read<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    let mut o = std::fs::OpenOptions::new();
    o.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.custom_flags(libc::O_NOFOLLOW);
    }
    let f = o.open(path).map_err(|_| "PRIVATE_FILE_UNAVAILABLE")?;
    let m = f.metadata().map_err(|_| "PRIVATE_FILE_INVALID")?;
    if !m.is_file() || m.len() > 131072 {
        return Err("PRIVATE_FILE_INVALID");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        if m.permissions().mode() & 0o077 != 0 || m.uid() != unsafe { libc::geteuid() } {
            return Err("PRIVATE_FILE_INVALID");
        }
    }
    serde_json::from_reader(f).map_err(|_| "PRIVATE_FILE_INVALID")
}
impl Policy {
    pub fn read(path: &Path) -> Result<Self> {
        if !path.is_absolute() {
            return Err("PRIVATE_FILE_INVALID");
        }
        let p: Self = read(path)?;
        if p.protocol != 3
            || !p.enabled
            || unhex::<32>(&p.server_public_key)
                .and_then(|b| VerifyingKey::from_bytes(&b).ok())
                .is_none()
        {
            return Err("INVALID_NOTEBOOK_POLICY");
        }
        Ok(p)
    }
}
fn atomic(path: &Path, value: &impl Serialize) -> Result<()> {
    use std::io::Write;
    let mut rand = [0; 16];
    getrandom::fill(&mut rand).map_err(|_| "RANDOM_UNAVAILABLE")?;
    let parent = path.parent().ok_or("PRIVATE_FILE_INVALID")?;
    let tmp = parent.join(format!(".notebook-{}.tmp", hex(&rand)));
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let mut f = o.open(&tmp).map_err(|_| "JOURNAL_UNAVAILABLE")?;
    f.write_all(&serde_json::to_vec(value).map_err(|_| "JOURNAL_INVALID")?)
        .map_err(|_| "JOURNAL_UNAVAILABLE")?;
    f.sync_all().map_err(|_| "JOURNAL_UNAVAILABLE")?;
    std::fs::rename(&tmp, path).map_err(|_| "JOURNAL_UNAVAILABLE")?;
    std::fs::File::open(parent)
        .and_then(|f| f.sync_all())
        .map_err(|_| "JOURNAL_UNAVAILABLE")
}
impl<T: Transport, D: Docker> Client<T, D> {
    pub fn open(
        transport: T,
        key: SigningKey,
        policy: Policy,
        path: &Path,
        node: &str,
        version: i64,
        executor: Arc<Executor<D>>,
    ) -> Result<Self> {
        if !path.is_absolute()
            || !valid_hardware_uuid(&format!("GPU-{node}"))
            || version <= 0
            || policy.protocol != 3
            || !policy.enabled
        {
            return Err("INVALID_NOTEBOOK_POLICY");
        }
        let state = if path.try_exists().map_err(|_| "JOURNAL_UNAVAILABLE")? {
            let j: Journal = read(path)?;
            if j.protocol != 3
                || j.node_id != node
                || j.version != version
                || j.server_key != policy.server_public_key
                || j.image != policy.image_digest
                || j.sequence < 0
                || j.active.len() > 16
                || j.seen.len() > 128
            {
                return Err("JOURNAL_BINDING_MISMATCH");
            }
            j
        } else {
            Journal {
                protocol: 3,
                node_id: node.into(),
                version,
                server_key: policy.server_public_key.clone(),
                image: policy.image_digest.clone(),
                sequence: 0,
                pending: None,
                active: vec![],
                seen: vec![],
            }
        };
        let c = Self {
            transport,
            key,
            policy,
            path: path.into(),
            state,
            executor,
        };
        for lease in c
            .state
            .active
            .iter()
            .chain(c.state.pending.iter().map(|p| &p.lease))
        {
            let now = time()?;
            let expires = lease.expires_at.min(lease.payload.deadline);
            if expires > now {
                c.executor
                    .authorize_until(&lease.payload.job_id, expires, now)?;
            } else {
                c.executor.invalidate(&lease.payload.job_id)?;
            }
        }
        c.save()?;
        Ok(c)
    }
    fn save(&self) -> Result<()> {
        atomic(&self.path, &self.state)
    }
    async fn request(&mut self, path: &str, payload: Payload) -> Result<Envelope> {
        self.state.sequence = self
            .state
            .sequence
            .checked_add(1)
            .ok_or("SEQUENCE_EXHAUSTED")?;
        self.save()?;
        let req = NodeRequest {
            protocol: 3,
            node_id: self.state.node_id.clone(),
            credential_version: self.state.version,
            sequence: self.state.sequence,
            issued_at: time()?,
            method: "POST".into(),
            path: path.into(),
            payload,
        };
        let signed = SignedNodeRequest {
            signature: sign_node_request(&req, &self.key),
            request: req,
        };
        let response = self.transport.send(path, &signed).await?;
        let now = time()?;
        if !verify_bound_envelope(
            &response,
            &self.state.node_id,
            self.state.version,
            self.state.sequence,
            "POST",
            path,
            &self.policy.server_public_key,
        ) || response.envelope.issued_at < now - 30
            || response.envelope.issued_at > now + 30
            || response.envelope.expires_at <= now
            || response.envelope.expires_at > now + 120
            || response.envelope.command.as_ref().is_some_and(|l| {
                l.lease_epoch <= 0
                    || unhex::<32>(&l.lease_token).is_none()
                    || l.expires_at <= now
                    || l.expires_at > now + 120
                    || l.payload.node_id != self.state.node_id
                    || !["START_NOTEBOOK", "STOP_NOTEBOOK"].contains(&l.kind.as_str())
            })
        {
            return Err("INVALID_CONTROL_AUTHORITY");
        }
        Ok(response.envelope)
    }
    pub async fn capabilities(&mut self, hardware_uuids: Vec<String>) -> Result<()> {
        if hardware_uuids.is_empty()
            || hardware_uuids.len() > 16
            || hardware_uuids.iter().any(|v| !valid_hardware_uuid(v))
        {
            return Err("NVIDIA_UNAVAILABLE");
        }
        self.executor.prove_hardware(&hardware_uuids).await?;
        self.request(
            "/v1/gpu/notebooks/capabilities",
            Payload::Capabilities {
                executor_protocol: 3,
                image_digest: self.policy.image_digest.clone(),
                docker_ready: true,
                toolkit_ready: true,
                hardware_uuids,
            },
        )
        .await?;
        Ok(())
    }
    pub async fn tick(&mut self) -> Result<()> {
        if let Some(lease) = self
            .state
            .pending
            .as_ref()
            .filter(|p| p.lease.expires_at <= time().unwrap_or(i64::MAX))
            .map(|p| p.lease.clone())
        {
            let response = self
                .request(
                    "/v1/gpu/notebooks/renew",
                    Payload::Renew {
                        command_id: lease.command_id.clone(),
                        lease_epoch: lease.lease_epoch,
                        lease_token: lease.lease_token.clone(),
                    },
                )
                .await?;
            if let Some(current) = response.command {
                if current.command_id != lease.command_id
                    || current.lease_epoch != lease.lease_epoch
                    || current.lease_token != lease.lease_token
                    || current.kind != lease.kind
                    || serde_json::to_value(&current.payload).ok()
                        != serde_json::to_value(&lease.payload).ok()
                {
                    return Err("INVALID_CONTROL_AUTHORITY");
                }
                if let Some(p) = self.state.pending.as_mut() {
                    p.lease = current;
                }
            } else {
                self.executor.stop_bound(&lease.payload).await?;
                self.state.seen.push((lease.command_id, lease.lease_epoch));
                if self.state.seen.len() > 128 {
                    self.state.seen.remove(0);
                }
                self.state.pending = None;
            }
            self.save()?;
        }
        if let Some(p) = &self.state.pending
            && let Some(result) = p.result.clone()
        {
            let lease = p.lease.clone();
            if result.state == "RUNNING" && !self.executor.is_running(&lease.payload.job_id).await?
            {
                self.state.seen.push((lease.command_id, lease.lease_epoch));
                if self.state.seen.len() > 128 {
                    self.state.seen.remove(0);
                }
                self.state.pending = None;
                return self.save();
            }
            self.request(
                "/v1/gpu/notebooks/result",
                Payload::Result {
                    command_id: lease.command_id.clone(),
                    lease_epoch: lease.lease_epoch,
                    lease_token: lease.lease_token.clone(),
                    result: result.clone(),
                },
            )
            .await?;
            if result.state == "RUNNING"
                && !self
                    .state
                    .active
                    .iter()
                    .any(|a| a.command_id == lease.command_id)
            {
                self.state.active.push(lease.clone())
            }
            self.state.seen.push((lease.command_id, lease.lease_epoch));
            if self.state.seen.len() > 128 {
                self.state.seen.remove(0);
            }
            self.state.pending = None;
            self.save()?;
        }
        for lease in self.state.active.clone() {
            if !self.executor.is_running(&lease.payload.job_id).await? {
                self.state
                    .active
                    .retain(|a| a.command_id != lease.command_id);
                self.save()?;
                continue;
            }
            let response = self
                .request(
                    "/v1/gpu/notebooks/renew",
                    Payload::Renew {
                        command_id: lease.command_id.clone(),
                        lease_epoch: lease.lease_epoch,
                        lease_token: lease.lease_token.clone(),
                    },
                )
                .await?;
            if let Some(current) = response.command {
                if current.command_id != lease.command_id
                    || current.lease_epoch != lease.lease_epoch
                    || current.lease_token != lease.lease_token
                    || serde_json::to_value(&current.payload).ok()
                        != serde_json::to_value(&lease.payload).ok()
                {
                    return Err("INVALID_CONTROL_AUTHORITY");
                }
                self.executor.authorize_until(
                    &current.payload.job_id,
                    current.expires_at.min(current.payload.deadline),
                    time()?,
                )?;
                if let Some(active) = self
                    .state
                    .active
                    .iter_mut()
                    .find(|a| a.command_id == current.command_id)
                {
                    *active = current;
                }
            } else {
                self.executor.stop_bound(&lease.payload).await?;
                self.state
                    .active
                    .retain(|a| a.command_id != lease.command_id);
                self.save()?;
            }
        }
        if self.state.pending.is_none()
            && let Some(lease) = self
                .request("/v1/gpu/notebooks/claim", Payload::Claim {})
                .await?
                .command
        {
            if self
                .state
                .seen
                .iter()
                .any(|(id, epoch)| id == &lease.command_id && *epoch >= lease.lease_epoch)
            {
                return Err("REPLAYED_CONTROL_COMMAND");
            }
            self.state.pending = Some(Pending {
                lease,
                result: None,
            });
            self.save()?;
        }
        if let Some(p) = &self.state.pending
            && p.result.is_none()
        {
            let mut lease = p.lease.clone();
            let now = time()?;
            if lease.expires_at <= now {
                let Some(renewed) = self
                    .request(
                        "/v1/gpu/notebooks/renew",
                        Payload::Renew {
                            command_id: lease.command_id.clone(),
                            lease_epoch: lease.lease_epoch,
                            lease_token: lease.lease_token.clone(),
                        },
                    )
                    .await?
                    .command
                else {
                    return Err("CONTROL_LEASE_EXPIRED");
                };
                if renewed.command_id != lease.command_id
                    || renewed.lease_epoch != lease.lease_epoch
                    || renewed.lease_token != lease.lease_token
                    || renewed.kind != lease.kind
                    || serde_json::to_value(&renewed.payload).ok()
                        != serde_json::to_value(&lease.payload).ok()
                {
                    return Err("INVALID_CONTROL_AUTHORITY");
                }
                lease = renewed;
            }
            if lease.kind == "START_NOTEBOOK" {
                self.executor.authorize_until(
                    &lease.payload.job_id,
                    lease.expires_at.min(lease.payload.deadline),
                    time()?,
                )?;
            }
            let outcome = if lease.kind == "START_NOTEBOOK" {
                self.executor.start(&lease.payload, time()?).await
            } else {
                self.executor.stop_bound(&lease.payload).await
            };
            let result = match outcome {
                Ok(value) => value,
                Err(code) => NotebookResult {
                    state: "FAILED".into(),
                    job_id: lease.payload.job_id.clone(),
                    generation: lease.payload.job_id.clone(),
                    error_code: Some(code.into()),
                },
            };
            self.state.pending = Some(Pending {
                lease,
                result: Some(result),
            });
            self.save()?;
        }
        self.save()
    }
}
