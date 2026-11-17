//! Domain-separated typed diagnostic protocol. No executable strings or arbitrary commands.
use crate::{GpuHealth, hex, unhex};
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
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
    pub payload: CommandPayload,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum CommandPayload {
    Claim {
        agent_protocol: u16,
    },
    Renew {
        command_id: String,
        lease_epoch: i64,
        lease_token: String,
    },
    Result {
        command_id: String,
        lease_epoch: i64,
        lease_token: String,
        result: DiagnosticResult,
    },
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiagnosticResult {
    pub status: String,
    pub gpus: Vec<GpuHealth>,
    pub error_code: Option<String>,
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
    pub payload: serde_json::Value,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommandEnvelope {
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
    pub envelope: CommandEnvelope,
    pub signature: String,
}
fn request_bytes(r: &NodeRequest) -> Vec<u8> {
    let hash = hex(&Sha256::digest(
        serde_json::to_vec(&r.payload).expect("typed payload"),
    ));
    format!(
        "CBGPU_NODE_COMMAND_V2\n{}\n{}\n{}\n{}",
        r.method,
        r.path,
        hash,
        serde_json::to_string(r).expect("typed request")
    )
    .into_bytes()
}
fn envelope_bytes(e: &CommandEnvelope) -> Vec<u8> {
    let mut bytes = b"CBGPU_CENTRAL_COMMAND_V2\n".to_vec();
    bytes.extend(serde_json::to_vec(e).expect("typed envelope"));
    bytes
}
fn verify(bytes: &[u8], signature: &str, key: &str) -> bool {
    let (Some(k), Some(s)) = (unhex::<32>(key), unhex::<64>(signature)) else {
        return false;
    };
    VerifyingKey::from_bytes(&k)
        .is_ok_and(|k| k.verify_strict(bytes, &Signature::from_bytes(&s)).is_ok())
}
pub fn sign_node_request(r: &NodeRequest, key: &SigningKey) -> String {
    hex(&key.sign(&request_bytes(r)).to_bytes())
}
pub fn verify_node_request(r: &NodeRequest, signature: &str, key: &str) -> bool {
    verify(&request_bytes(r), signature, key)
}
pub fn sign_envelope(envelope: CommandEnvelope, key: &SigningKey) -> SignedEnvelope {
    let signature = hex(&key.sign(&envelope_bytes(&envelope)).to_bytes());
    SignedEnvelope {
        envelope,
        signature,
    }
}
pub fn verify_envelope(s: &SignedEnvelope, node: &str, sequence: i64, key: &str) -> bool {
    s.envelope.protocol == 2
        && s.envelope.node_id == node
        && s.envelope.request_sequence == sequence
        && verify(&envelope_bytes(&s.envelope), &s.signature, key)
}
pub fn verify_bound_envelope(
    s: &SignedEnvelope,
    node: &str,
    sequence: i64,
    method: &str,
    path: &str,
    key: &str,
) -> bool {
    verify_envelope(s, node, sequence, key)
        && s.envelope.request_method == method
        && s.envelope.request_path == path
}
pub fn valid_result(r: &DiagnosticResult) -> bool {
    match r.status.as_str() {
        "FAILED" => {
            r.gpus.is_empty()
                && r.error_code.as_deref().is_some_and(|x| {
                    [
                        "NVIDIA_UNAVAILABLE",
                        "INVALID_INVENTORY",
                        "DIAGNOSTIC_TIMEOUT",
                    ]
                    .contains(&x)
                })
        }
        "SUCCEEDED" => {
            r.error_code.is_none()
                && (1..=16).contains(&r.gpus.len())
                && r.gpus.iter().enumerate().all(|(i, g)| {
                    g.hardware_uuid
                        .as_deref()
                        .is_some_and(crate::valid_hardware_uuid)
                        && !g.name.is_empty()
                        && g.name.len() <= 120
                        && !g.name.chars().any(char::is_control)
                        && g.index < 256
                        && g.memory_total_mib > 0
                        && g.memory_total_mib <= 16777216
                        && g.memory_used_mib <= g.memory_total_mib
                        && g.utilization_percent <= 100
                        && !r.gpus[..i]
                            .iter()
                            .any(|o| o.index == g.index || o.hardware_uuid == g.hardware_uuid)
                })
        }
        _ => false,
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn domains_paths_and_sequences_cannot_be_replayed() {
        let k = SigningKey::from_bytes(&[4; 32]);
        let key = hex(k.verifying_key().as_bytes());
        let mut r = NodeRequest {
            protocol: 2,
            node_id: "node".into(),
            credential_version: 1,
            sequence: 1,
            issued_at: 1,
            method: "POST".into(),
            path: "/v1/gpu/commands/claim".into(),
            payload: CommandPayload::Claim { agent_protocol: 2 },
        };
        let s = sign_node_request(&r, &k);
        assert!(verify_node_request(&r, &s, &key));
        r.path = "/v1/gpu/commands/result".into();
        assert!(!verify_node_request(&r, &s, &key));
        let e = sign_envelope(
            CommandEnvelope {
                protocol: 2,
                credential_version: 1,
                node_id: "node".into(),
                request_sequence: 1,
                request_method: "POST".into(),
                request_path: "/v1/gpu/commands/claim".into(),
                issued_at: 1,
                expires_at: 60,
                command: None,
            },
            &k,
        );
        assert!(verify_envelope(&e, "node", 1, &key));
        assert!(!verify_envelope(&e, "node", 2, &key));
        assert!(!verify_envelope(&e, "other", 1, &key));
    }
}
