//! Authenticated outbound notebook transport. Contains no execution or routing authority.
use base64::{Engine, engine::general_purpose::STANDARD};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
pub const MAX_HTTP_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_WS_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_RELAY_MESSAGE: usize = 12 * 1024 * 1024;
pub fn uuid(s: &str) -> bool {
    s.len() == 36 && uuid::Uuid::parse_str(s).is_ok()
}
pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
pub fn unhex<const N: usize>(s: &str) -> Option<[u8; N]> {
    if s.len() != N * 2 {
        return None;
    }
    let mut out = [0; N];
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(s.get(i * 2..i * 2 + 2)?, 16).ok()?
    }
    Some(out)
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Challenge {
    pub node_id: String,
    pub credential_version: i64,
    pub nonce: String,
    pub issued_at: i64,
    pub expires_at: i64,
    pub server_signature: String,
}
impl Challenge {
    pub fn bytes(&self) -> Vec<u8> {
        format!(
            "cbgpu-notebook-relay-challenge-v1\n{}\n{}\n{}\n{}\n{}",
            self.node_id, self.credential_version, self.nonce, self.issued_at, self.expires_at
        )
        .into_bytes()
    }
    pub fn issue(node: String, version: i64, nonce: [u8; 32], now: i64, key: &SigningKey) -> Self {
        let mut c = Self {
            node_id: node,
            credential_version: version,
            nonce: hex(&nonce),
            issued_at: now,
            expires_at: now.saturating_add(20),
            server_signature: String::new(),
        };
        c.server_signature = hex(&key.sign(&c.bytes()).to_bytes());
        c
    }
    pub fn verify_server(&self, key: &VerifyingKey, now: i64) -> bool {
        uuid(&self.node_id)
            && self.credential_version > 0
            && unhex::<32>(&self.nonce).is_some()
            && self
                .expires_at
                .checked_sub(self.issued_at)
                .is_some_and(|d| (1..=30).contains(&d))
            && now
                .checked_sub(self.issued_at)
                .and_then(i64::checked_abs)
                .is_some_and(|d| d <= 90)
            && now <= self.expires_at.saturating_add(90)
            && unhex::<64>(&self.server_signature).is_some_and(|s| {
                key.verify(&self.bytes(), &Signature::from_bytes(&s))
                    .is_ok()
            })
    }
    pub fn node_bytes(&self) -> Vec<u8> {
        format!(
            "cbgpu-notebook-relay-node-v1\n{}",
            hex(&Sha256::digest(self.bytes()))
        )
        .into_bytes()
    }
    pub fn node_signature(&self, key: &SigningKey) -> String {
        hex(&key.sign(&self.node_bytes()).to_bytes())
    }
    pub fn verify_node(&self, key: &VerifyingKey, signature: &str, now: i64) -> bool {
        now >= self.issued_at
            && now <= self.expires_at
            && unhex::<64>(signature).is_some_and(|s| {
                key.verify(&self.node_bytes(), &Signature::from_bytes(&s))
                    .is_ok()
            })
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Frame {
    Challenge {
        challenge: Challenge,
    },
    Auth {
        signature: String,
    },
    Ready {},
    HttpRequest {
        stream_id: String,
        job_id: String,
        relay_session_id: String,
        method: String,
        uri: String,
        headers: Vec<(String, String)>,
        body: String,
    },
    HttpResponse {
        stream_id: String,
        status: u16,
        headers: Vec<(String, String)>,
        body: String,
    },
    WsOpen {
        stream_id: String,
        job_id: String,
        relay_session_id: String,
        uri: String,
        headers: Vec<(String, String)>,
        protocols: Vec<String>,
    },
    WsOpened {
        stream_id: String,
        protocol: Option<String>,
    },
    WsData {
        stream_id: String,
        binary: bool,
        body: String,
    },
    Close {
        stream_id: String,
        code: u16,
    },
    Error {
        stream_id: String,
        code: String,
    },
    Revoked {},
    Ping {},
    Pong {},
}
impl std::fmt::Debug for Frame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("NotebookRelayFrame(private payload redacted)")
    }
}
pub fn decode_frame(text: &str) -> Result<Frame, &'static str> {
    if text.len() > MAX_RELAY_MESSAGE {
        return Err("FRAME_TOO_LARGE");
    }
    serde_json::from_str(text).map_err(|_| "INVALID_FRAME")
}
pub fn encode_frame(frame: &Frame) -> Result<String, &'static str> {
    let text = serde_json::to_string(frame).map_err(|_| "INVALID_FRAME")?;
    if text.len() > MAX_RELAY_MESSAGE {
        return Err("FRAME_TOO_LARGE");
    }
    Ok(text)
}
pub fn headers_valid(headers: &[(String, String)]) -> bool {
    headers.len() <= 32
        && headers.iter().all(|(k, v)| {
            !k.is_empty()
                && k.len() <= 64
                && k.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
                && v.len() <= 8192
                && !v.chars().any(|c| c == '\r' || c == '\n' || c == '\0')
        })
        && headers
            .iter()
            .map(|(k, v)| k.len() + v.len())
            .sum::<usize>()
            <= 32768
}
pub fn decode_body(body: &str, limit: usize) -> Option<Vec<u8>> {
    if body.len() > limit.div_ceil(3) * 4 {
        return None;
    }
    let bytes = STANDARD.decode(body).ok()?;
    (bytes.len() <= limit).then_some(bytes)
}
pub fn encode_body(bytes: &[u8]) -> String {
    STANDARD.encode(bytes)
}
pub fn notebook_uri(job: &str, uri: &str) -> bool {
    if !uuid(job) || uri.len() > 8192 || uri.chars().any(|c| c.is_control() || c == '\\') {
        return false;
    }
    let prefix = format!("/notebooks/{job}/");
    if !uri.starts_with(&prefix) {
        return false;
    }
    let Ok(url) = url::Url::parse(&format!("http://relay.invalid{uri}")) else {
        return false;
    };
    url.host_str() == Some("relay.invalid")
        && url.path().starts_with(&prefix)
        && url.fragment().is_none()
}
#[cfg(test)]
mod tests {
    use super::*;
    const JOB: &str = "11111111-1111-4111-8111-111111111111";
    #[test]
    fn signatures_bind_both_keys_and_every_connection_challenge() {
        let server = SigningKey::from_bytes(&[1; 32]);
        let node = SigningKey::from_bytes(&[2; 32]);
        let c = Challenge::issue(JOB.into(), 1, [3; 32], 100, &server);
        assert!(c.verify_server(&server.verifying_key(), 100));
        let proof = c.node_signature(&node);
        assert!(c.verify_node(&node.verifying_key(), &proof, 110));
        assert!(!c.verify_node(&server.verifying_key(), &proof, 110));
        assert!(!c.verify_node(&node.verifying_key(), &proof, 121));
        let other = Challenge::issue(JOB.into(), 1, [4; 32], 100, &server);
        assert!(!other.verify_node(&node.verifying_key(), &proof, 110));
        let mut bad = c.clone();
        bad.credential_version = 2;
        assert!(!bad.verify_server(&server.verifying_key(), 100));
    }
    #[test]
    fn bounded_bodies_headers_and_paths_do_not_become_proxy_destinations() {
        assert!(notebook_uri(
            JOB,
            &format!("/notebooks/{JOB}/api/kernels?q=one")
        ));
        for s in [
            "https://evil.invalid/",
            "//evil.invalid/",
            &format!("/notebooks/{JOB}/../../private"),
            &format!("/notebooks/{JOB}/%2e%2e/%2e%2e/private"),
        ] {
            assert!(!notebook_uri(JOB, s))
        }
        assert!(headers_valid(&[(
            "Content-Type".into(),
            "application/json".into()
        )]));
        assert!(!headers_valid(&[(
            "X-Test".into(),
            "x\r\nAuthorization: bad".into()
        )]));
        assert_eq!(decode_body(&encode_body(b"ok"), 2), Some(b"ok".to_vec()));
        assert!(decode_body(&encode_body(b"long"), 2).is_none());
    }
}
