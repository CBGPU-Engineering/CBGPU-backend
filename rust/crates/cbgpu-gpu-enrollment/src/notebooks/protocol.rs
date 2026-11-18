//! Notebook protocol v3. Fixed typed operations, never executable strings.
use crate::{hex, unhex};
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Limits {
    pub cpu_millis: u32,
    pub memory_bytes: u64,
    pub pids: u32,
    pub tmpfs_bytes: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NotebookSpec {
    pub job_id: String,
    pub school_id: String,
    pub node_id: String,
    pub resource_id: String,
    pub hardware_uuid: String,
    pub reservation_id: String,
    pub owner_id: String,
    pub not_before: i64,
    pub deadline: i64,
    pub image_digest: String,
    pub limits: Limits,
    pub relay_session_id: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NotebookResult {
    pub state: String,
    pub job_id: String,
    pub generation: String,
    pub error_code: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum Payload {
    Capabilities {
        executor_protocol: u16,
        image_digest: String,
        docker_ready: bool,
        toolkit_ready: bool,
        hardware_uuids: Vec<String>,
    },
    Claim {},
    Renew {
        command_id: String,
        lease_epoch: i64,
        lease_token: String,
    },
    Result {
        command_id: String,
        lease_epoch: i64,
        lease_token: String,
        result: NotebookResult,
    },
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NodeRequest {
    pub protocol: u16,
    pub node_id: String,
    pub credential_version: i64,
    pub sequence: i64,
    pub issued_at: i64,
    pub method: String,
    pub path: String,
    pub payload: Payload,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignedNodeRequest {
    pub request: NodeRequest,
    pub signature: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommandLease {
    pub command_id: String,
    pub lease_epoch: i64,
    pub lease_token: String,
    pub expires_at: i64,
    pub kind: String,
    pub payload: NotebookSpec,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Envelope {
    pub protocol: u16,
    pub credential_version: i64,
    pub node_id: String,
    pub request_sequence: i64,
    pub request_method: String,
    pub request_path: String,
    pub issued_at: i64,
    pub expires_at: i64,
    pub command: Option<CommandLease>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignedEnvelope {
    pub envelope: Envelope,
    pub signature: String,
}
fn request_bytes(r: &NodeRequest) -> Vec<u8> {
    format!(
        "CBGPU_NOTEBOOK_NODE_V3\n{}\n{}\n{}\n{}",
        r.method,
        r.path,
        hex(&Sha256::digest(
            serde_json::to_vec(&r.payload).expect("typed")
        )),
        serde_json::to_string(r).expect("typed")
    )
    .into_bytes()
}
fn envelope_bytes(e: &Envelope) -> Vec<u8> {
    let mut b = b"CBGPU_NOTEBOOK_CENTRAL_V3\n".to_vec();
    b.extend(serde_json::to_vec(e).expect("typed"));
    b
}
fn verify(b: &[u8], s: &str, k: &str) -> bool {
    let (Some(k), Some(s)) = (unhex::<32>(k), unhex::<64>(s)) else {
        return false;
    };
    VerifyingKey::from_bytes(&k)
        .is_ok_and(|k| k.verify_strict(b, &Signature::from_bytes(&s)).is_ok())
}
pub fn sign_node_request(r: &NodeRequest, k: &SigningKey) -> String {
    hex(&k.sign(&request_bytes(r)).to_bytes())
}
pub fn verify_node_request(r: &NodeRequest, s: &str, k: &str) -> bool {
    verify(&request_bytes(r), s, k)
}
pub fn sign_envelope(envelope: Envelope, k: &SigningKey) -> SignedEnvelope {
    let signature = hex(&k.sign(&envelope_bytes(&envelope)).to_bytes());
    SignedEnvelope {
        envelope,
        signature,
    }
}
pub fn verify_envelope(s: &SignedEnvelope, node: &str, sequence: i64, key: &str) -> bool {
    s.envelope.protocol == 3
        && s.envelope.node_id == node
        && s.envelope.request_sequence == sequence
        && verify(&envelope_bytes(&s.envelope), &s.signature, key)
}
pub fn verify_bound_envelope(
    s: &SignedEnvelope,
    node: &str,
    version: i64,
    sequence: i64,
    method: &str,
    path: &str,
    key: &str,
) -> bool {
    verify_envelope(s, node, sequence, key)
        && s.envelope.credential_version == version
        && s.envelope.request_method == method
        && s.envelope.request_path == path
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn signatures_bind_path_payload_epoch_and_domain() {
        let k = SigningKey::from_bytes(&[9; 32]);
        let key = hex(k.verifying_key().as_bytes());
        let mut r = NodeRequest {
            protocol: 3,
            node_id: "node".into(),
            credential_version: 1,
            sequence: 2,
            issued_at: 1,
            method: "POST".into(),
            path: "/v1/gpu/notebooks/claim".into(),
            payload: Payload::Claim {},
        };
        let sig = sign_node_request(&r, &k);
        assert!(verify_node_request(&r, &sig, &key));
        r.path.push('x');
        assert!(!verify_node_request(&r, &sig, &key));
        r.path.pop();
        r.payload = Payload::Renew {
            command_id: "other".into(),
            lease_epoch: 2,
            lease_token: "token".into(),
        };
        assert!(!verify_node_request(&r, &sig, &key));
    }
}
