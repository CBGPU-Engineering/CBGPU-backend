//! Optional signed diagnostic control. Private journal is separate from legacy health sequence.
use super::diagnostics::{self, Runner};
use cbgpu_gpu_enrollment::{
    commands::protocol::{
        CommandEnvelope, CommandLease, CommandPayload, DiagnosticResult, NodeRequest,
        SignedEnvelope, SignedNodeRequest, sign_node_request, valid_result, verify_bound_envelope,
    },
    hex, unhex, valid_hardware_uuid,
};
use ed25519_dalek::{SigningKey, VerifyingKey};
use serde::{Deserialize, Serialize};
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};
const CLAIM: &str = "/v1/gpu/commands/claim";
const RENEW: &str = "/v1/gpu/commands/renew";
const RESULT: &str = "/v1/gpu/commands/result";
pub type Error = &'static str;
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Trust {
    protocol: u16,
    server_public_key: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Seen {
    command_id: String,
    lease_epoch: i64,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Pending {
    lease: CommandLease,
    started: bool,
    result: Option<DiagnosticResult>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Journal {
    protocol: u16,
    node_id: String,
    credential_version: i64,
    server_public_key: String,
    sequence: i64,
    pending: Option<Pending>,
    seen: Vec<Seen>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Idle,
    Submitted,
    Interrupted,
}
pub struct Client {
    url: String,
    node_id: String,
    version: i64,
    key: SigningKey,
    server_key: String,
    journal: PathBuf,
    state: Journal,
    http: reqwest::Client,
}
fn now() -> Result<i64, Error> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| "CLOCK_UNAVAILABLE")?
        .as_secs()
        .try_into()
        .map_err(|_| "CLOCK_UNAVAILABLE")
}
fn private_read(path: &Path) -> Result<Vec<u8>, Error> {
    if !path.is_absolute() {
        return Err("PRIVATE_ABSOLUTE_FILE_REQUIRED");
    }
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let mut file = options.open(path).map_err(|_| "PRIVATE_FILE_UNAVAILABLE")?;
    let metadata = file.metadata().map_err(|_| "PRIVATE_FILE_UNAVAILABLE")?;
    if !metadata.is_file() || metadata.len() > 65536 {
        return Err("PRIVATE_FILE_INVALID");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.mode() & 0o077 != 0 || metadata.uid() != unsafe { libc::geteuid() } {
            return Err("PRIVATE_FILE_PERMISSIONS");
        }
    }
    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take(65537)
        .read_to_end(&mut bytes)
        .map_err(|_| "PRIVATE_FILE_UNAVAILABLE")?;
    if bytes.len() > 65536 {
        return Err("PRIVATE_FILE_INVALID");
    }
    Ok(bytes)
}
fn atomic(path: &Path, value: &Journal, new: bool) -> Result<(), Error> {
    let parent = path.parent().ok_or("PRIVATE_FILE_INVALID")?;
    let meta = std::fs::metadata(parent).map_err(|_| "PRIVATE_FILE_UNAVAILABLE")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if meta.mode() & 0o022 != 0 || meta.uid() != unsafe { libc::geteuid() } {
            return Err("PRIVATE_DIRECTORY_PERMISSIONS");
        }
    }
    if !new {
        private_read(path)?;
    }
    let mut nonce = [0; 16];
    getrandom::fill(&mut nonce).map_err(|_| "RANDOM_UNAVAILABLE")?;
    let temp = parent.join(format!(".diagnostic-{}.tmp", hex(&nonce)));
    let mut options = std::fs::OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let result = (|| {
        let mut file = options.open(&temp).map_err(|_| "JOURNAL_UNAVAILABLE")?;
        file.write_all(&serde_json::to_vec(value).map_err(|_| "JOURNAL_INVALID")?)
            .map_err(|_| "JOURNAL_UNAVAILABLE")?;
        file.sync_all().map_err(|_| "JOURNAL_UNAVAILABLE")?;
        if new {
            std::fs::hard_link(&temp, path).map_err(|_| "JOURNAL_ALREADY_EXISTS")?;
        } else {
            std::fs::rename(&temp, path).map_err(|_| "JOURNAL_UNAVAILABLE")?;
        }
        std::fs::File::open(parent)
            .and_then(|file| file.sync_all())
            .map_err(|_| "JOURNAL_UNAVAILABLE")?;
        Ok(())
    })();
    let _ = std::fs::remove_file(temp);
    result
}
fn lease_valid(lease: &CommandLease, time: i64) -> bool {
    valid_hardware_uuid(&format!("GPU-{}", lease.command_id))
        && lease.lease_epoch > 0
        && unhex::<32>(&lease.lease_token).is_some()
        && lease.kind == "diagnostic_v1"
        && lease.payload.as_object().is_some_and(|v| v.is_empty())
        && lease.expires_at > time
        && lease.expires_at <= time.saturating_add(120)
}
impl Client {
    pub fn open(
        identity_path: &Path,
        trust_path: &Path,
        url: &str,
        node_id: &str,
        version: i64,
        key: SigningKey,
        http: reqwest::Client,
    ) -> Result<Self, Error> {
        let parsed = url::Url::parse(url).map_err(|_| "INVALID_REGISTRY")?;
        if parsed.scheme() != "https"
            || parsed.port_or_known_default() != Some(443)
            || parsed.host_str().is_none()
            || parsed.path() != "/"
            || parsed.query().is_some()
            || parsed.fragment().is_some()
            || !parsed.username().is_empty()
            || parsed.password().is_some()
        {
            return Err("INVALID_REGISTRY");
        }
        Self::open_trusted(identity_path, trust_path, url, node_id, version, key, http)
    }
    fn open_trusted(
        identity_path: &Path,
        trust_path: &Path,
        url: &str,
        node_id: &str,
        version: i64,
        key: SigningKey,
        http: reqwest::Client,
    ) -> Result<Self, Error> {
        let trust: Trust = serde_json::from_slice(&private_read(trust_path)?)
            .map_err(|_| "INVALID_SERVER_TRUST")?;
        if trust.protocol != 2
            || version <= 0
            || !valid_hardware_uuid(&format!("GPU-{node_id}"))
            || unhex::<32>(&trust.server_public_key)
                .and_then(|bytes| VerifyingKey::from_bytes(&bytes).ok())
                .is_none()
        {
            return Err("INVALID_SERVER_TRUST");
        }
        let mut name = identity_path.as_os_str().to_owned();
        name.push(".diagnostics.json");
        let journal = PathBuf::from(name);
        let state = if journal.try_exists().map_err(|_| "JOURNAL_UNAVAILABLE")? {
            let state: Journal =
                serde_json::from_slice(&private_read(&journal)?).map_err(|_| "JOURNAL_INVALID")?;
            if state.protocol != 2
                || state.node_id != node_id
                || state.credential_version != version
                || state.server_public_key != trust.server_public_key
                || state.sequence < 0
                || state.seen.len() > 64
            {
                return Err("JOURNAL_BINDING_MISMATCH");
            }
            if state
                .pending
                .as_ref()
                .is_some_and(|p| p.result.as_ref().is_some_and(|r| !valid_result(r)))
            {
                return Err("JOURNAL_INVALID");
            }
            state
        } else {
            let value = Journal {
                protocol: 2,
                node_id: node_id.into(),
                credential_version: version,
                server_public_key: trust.server_public_key.clone(),
                sequence: 0,
                pending: None,
                seen: Vec::new(),
            };
            atomic(&journal, &value, true)?;
            value
        };
        Ok(Self {
            url: url.trim_end_matches('/').into(),
            node_id: node_id.into(),
            version,
            key,
            server_key: trust.server_public_key,
            journal,
            state,
            http,
        })
    }
    pub fn matches(&self, url: &str, node: &str, version: i64, key: &SigningKey) -> bool {
        self.url == url.trim_end_matches('/')
            && self.node_id == node
            && self.version == version
            && self.key.verifying_key() == key.verifying_key()
    }
    fn save(&self) -> Result<(), Error> {
        atomic(&self.journal, &self.state, false)
    }
    async fn request(
        &mut self,
        path: &str,
        payload: CommandPayload,
    ) -> Result<CommandEnvelope, Error> {
        self.state.sequence = self
            .state
            .sequence
            .checked_add(1)
            .ok_or("SEQUENCE_EXHAUSTED")?;
        self.save()?;
        let request = NodeRequest {
            protocol: 2,
            node_id: self.node_id.clone(),
            credential_version: self.version,
            sequence: self.state.sequence,
            issued_at: now()?,
            method: "POST".into(),
            path: path.into(),
            payload,
        };
        let signed = SignedNodeRequest {
            signature: sign_node_request(&request, &self.key),
            request,
        };
        let mut response = self
            .http
            .post(format!("{}{path}", self.url))
            .json(&signed)
            .send()
            .await
            .map_err(|_| "CONTROL_UNAVAILABLE")?;
        if !response.status().is_success() {
            return Err("CONTROL_REJECTED");
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| "CONTROL_UNAVAILABLE")? {
            if bytes.len() + chunk.len() > 65536 {
                return Err("CONTROL_RESPONSE_TOO_LARGE");
            }
            bytes.extend_from_slice(&chunk)
        }
        let envelope: SignedEnvelope =
            serde_json::from_slice(&bytes).map_err(|_| "INVALID_CONTROL_RESPONSE")?;
        let time = now()?;
        if !verify_bound_envelope(
            &envelope,
            &self.node_id,
            self.state.sequence,
            "POST",
            path,
            &self.server_key,
        ) || envelope.envelope.credential_version != self.version
            || envelope.envelope.issued_at < time.saturating_sub(30)
            || envelope.envelope.issued_at > time.saturating_add(30)
            || envelope.envelope.expires_at <= time
            || envelope.envelope.expires_at > time.saturating_add(120)
            || envelope
                .envelope
                .command
                .as_ref()
                .is_some_and(|lease| !lease_valid(lease, time))
        {
            return Err("INVALID_CONTROL_AUTHORITY");
        }
        Ok(envelope.envelope)
    }
    fn complete(&mut self) -> Result<(), Error> {
        if let Some(pending) = self.state.pending.take() {
            self.state.seen.push(Seen {
                command_id: pending.lease.command_id,
                lease_epoch: pending.lease.lease_epoch,
            });
            if self.state.seen.len() > 64 {
                self.state.seen.remove(0);
            }
        }
        self.save()
    }
    pub async fn tick(&mut self, runner: Arc<dyn Runner>) -> Result<Outcome, Error> {
        if self
            .state
            .pending
            .as_ref()
            .is_some_and(|p| p.lease.expires_at <= now().unwrap_or(i64::MAX))
        {
            self.complete()?;
        }
        if self.state.pending.is_none() {
            let response = self
                .request(CLAIM, CommandPayload::Claim { agent_protocol: 2 })
                .await?;
            let Some(lease) = response.command else {
                return Ok(Outcome::Idle);
            };
            if self.state.seen.iter().any(|seen| {
                seen.command_id == lease.command_id && seen.lease_epoch >= lease.lease_epoch
            }) {
                return Err("REPLAYED_COMMAND");
            };
            self.state.pending = Some(Pending {
                lease,
                started: false,
                result: None,
            });
            self.save()?;
        }
        if self
            .state
            .pending
            .as_ref()
            .is_some_and(|p| p.started && p.result.is_none())
        {
            return Ok(Outcome::Interrupted);
        }
        if self
            .state
            .pending
            .as_ref()
            .is_some_and(|p| p.result.is_none())
        {
            let old = self
                .state
                .pending
                .as_ref()
                .ok_or("JOURNAL_INVALID")?
                .lease
                .clone();
            let response = self
                .request(
                    RENEW,
                    CommandPayload::Renew {
                        command_id: old.command_id.clone(),
                        lease_epoch: old.lease_epoch,
                        lease_token: old.lease_token.clone(),
                    },
                )
                .await?;
            let lease = response.command.ok_or("LEASE_WITHDRAWN")?;
            if lease.command_id != old.command_id
                || lease.lease_epoch != old.lease_epoch
                || lease.lease_token != old.lease_token
                || lease.expires_at < now()?.saturating_add(10)
            {
                return Err("LEASE_CHANGED");
            }
            let pending = self.state.pending.as_mut().ok_or("JOURNAL_INVALID")?;
            pending.lease = lease;
            pending.started = true;
            self.save()?;
            let result = tokio::task::spawn_blocking(move || diagnostics::result(runner.as_ref()))
                .await
                .map_err(|_| "DIAGNOSTIC_INTERRUPTED")?;
            if !valid_result(&result) {
                return Err("INVALID_DIAGNOSTIC_RESULT");
            }
            self.state.pending.as_mut().ok_or("JOURNAL_INVALID")?.result = Some(result);
            self.save()?;
        }
        let pending = self.state.pending.as_ref().ok_or("JOURNAL_INVALID")?;
        if pending.lease.expires_at <= now()? {
            return Err("LEASE_EXPIRED");
        }
        let payload = CommandPayload::Result {
            command_id: pending.lease.command_id.clone(),
            lease_epoch: pending.lease.lease_epoch,
            lease_token: pending.lease.lease_token.clone(),
            result: pending.result.clone().ok_or("JOURNAL_INVALID")?,
        };
        let response = self.request(RESULT, payload).await?;
        if response.command.is_some() {
            return Err("INVALID_RESULT_ACK");
        }
        self.complete()?;
        Ok(Outcome::Submitted)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cbgpu_gpu_enrollment::commands::protocol::sign_envelope;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    struct Fake(Arc<AtomicUsize>);
    impl Runner for Fake {
        fn query(&self) -> Result<Vec<cbgpu_gpu_enrollment::GpuHealth>, diagnostics::Failure> {
            self.0.fetch_add(1, Ordering::SeqCst);
            diagnostics::parse(
                b"0, Synthetic, 8192, 0, 0, GPU-11111111-1111-4111-8111-111111111111\n",
            )
        }
    }
    #[tokio::test]
    async fn pinned_https_lost_ack_retries_exact_result_without_reexecution() {
        let generated = rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
        let cert = generated.cert.der().clone();
        let key = rustls_key(generated.key_pair.serialize_der());
        let config = tokio_rustls::rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(vec![cert.clone()], key)
            .unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let signing = SigningKey::from_bytes(&[7; 32]);
        let server_key = hex(&signing.verifying_key().to_bytes());
        let node_key = SigningKey::from_bytes(&[8; 32]);
        let node_public = hex(&node_key.verifying_key().to_bytes());
        let server = tokio::spawn(async move {
            let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));
            let mut prior_sequence = 0;
            let mut previous_result = None;
            for attempt in 0..4 {
                let (tcp, _) = listener.accept().await.unwrap();
                let mut tls = acceptor.accept(tcp).await.unwrap();
                let mut bytes = Vec::new();
                let header_end = loop {
                    let mut b = [0; 1024];
                    let n = tls.read(&mut b).await.unwrap();
                    assert!(n > 0);
                    bytes.extend_from_slice(&b[..n]);
                    assert!(bytes.len() < 65536);
                    if let Some(at) = bytes.windows(4).position(|v| v == b"\r\n\r\n") {
                        break at + 4;
                    }
                };
                let headers = String::from_utf8(bytes[..header_end].to_vec()).unwrap();
                let length: usize = headers
                    .lines()
                    .find_map(|l| {
                        l.to_ascii_lowercase()
                            .strip_prefix("content-length:")
                            .map(|v| v.trim().parse().unwrap())
                    })
                    .unwrap();
                while bytes.len() < header_end + length {
                    let mut b = [0; 4096];
                    let n = tls.read(&mut b).await.unwrap();
                    assert!(n > 0);
                    bytes.extend_from_slice(&b[..n]);
                }
                let signed: SignedNodeRequest =
                    serde_json::from_slice(&bytes[header_end..header_end + length]).unwrap();
                assert!(
                    cbgpu_gpu_enrollment::commands::protocol::verify_node_request(
                        &signed.request,
                        &signed.signature,
                        &node_public
                    )
                );
                assert!(signed.request.sequence > prior_sequence);
                prior_sequence = signed.request.sequence;
                let command = match &signed.request.payload {
                    CommandPayload::Claim { .. } | CommandPayload::Renew { .. } => {
                        Some(CommandLease {
                            command_id: "22222222-2222-4222-8222-222222222222".into(),
                            lease_epoch: 1,
                            lease_token: hex(&[9; 32]),
                            expires_at: now().unwrap() + 60,
                            kind: "diagnostic_v1".into(),
                            payload: serde_json::json!({}),
                        })
                    }
                    CommandPayload::Result { result, .. } => {
                        let value = serde_json::to_value(result).unwrap();
                        if let Some(prior) = &previous_result {
                            assert_eq!(prior, &value)
                        }
                        previous_result = Some(value);
                        None
                    }
                };
                // Simulate transport loss AFTER accepting the first result.
                if attempt == 2 {
                    drop(tls);
                    continue;
                }
                let envelope = sign_envelope(
                    CommandEnvelope {
                        protocol: 2,
                        node_id: signed.request.node_id,
                        credential_version: 1,
                        request_sequence: signed.request.sequence,
                        request_method: "POST".into(),
                        request_path: signed.request.path,
                        issued_at: now().unwrap(),
                        expires_at: now().unwrap() + 90,
                        command,
                    },
                    &signing,
                );
                let body = serde_json::to_vec(&envelope).unwrap();
                let head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                tls.write_all(head.as_bytes()).await.unwrap();
                tls.write_all(&body).await.unwrap();
                tls.shutdown().await.unwrap();
            }
        });
        let mut random = [0; 16];
        getrandom::fill(&mut random).unwrap();
        let dir = std::env::temp_dir().join(format!("cbgpu-diagnostic-{}", hex(&random)));
        std::fs::create_dir(&dir).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        let trust = dir.join("trust.json");
        std::fs::write(
            &trust,
            serde_json::json!({"protocol":2,"server_public_key":server_key}).to_string(),
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&trust, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        let http = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(std::time::Duration::from_secs(5))
            .add_root_certificate(reqwest::Certificate::from_der(cert.as_ref()).unwrap())
            .build()
            .unwrap();
        let mut client = Client::open_trusted(
            &dir.join("identity.json"),
            &trust,
            &format!("https://localhost:{port}"),
            "33333333-3333-4333-8333-333333333333",
            1,
            node_key,
            http,
        )
        .unwrap();
        let count = Arc::new(AtomicUsize::new(0));
        let runner: Arc<dyn Runner> = Arc::new(Fake(count.clone()));
        assert_eq!(
            client.tick(runner.clone()).await,
            Err("CONTROL_UNAVAILABLE")
        );
        assert_eq!(count.load(Ordering::SeqCst), 1);
        assert_eq!(client.tick(runner).await, Ok(Outcome::Submitted));
        assert_eq!(count.load(Ordering::SeqCst), 1);
        server.await.unwrap();
        std::fs::remove_dir_all(dir).unwrap();
    }
    fn rustls_key(bytes: Vec<u8>) -> tokio_rustls::rustls::pki_types::PrivateKeyDer<'static> {
        tokio_rustls::rustls::pki_types::PrivatePkcs8KeyDer::from(bytes).into()
    }
}
