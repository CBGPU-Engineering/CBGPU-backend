//! Separately reviewed, default-off contest-scoped delegation. Frozen JWT service is unchanged.
pub mod api;
pub mod store;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
pub use cbgpu_contests_api::{Error, Identity, RegistrationInput, Result, fail};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
#[derive(Clone)]
pub struct TokenHash([u8; 32]);
impl TokenHash {
    pub fn parse(secret: &str) -> Result<Self> {
        let raw = secret
            .strip_prefix("cbgpu_ct_")
            .filter(|v| v.len() == 43)
            .ok_or_else(|| {
                fail(
                    401,
                    "invalid_contest_token",
                    "Use a scoped CBGPU contest token",
                )
            })?;
        if URL_SAFE_NO_PAD
            .decode(raw)
            .ok()
            .is_none_or(|v| v.len() != 32)
        {
            return Err(fail(401, "invalid_contest_token", "Invalid contest token"));
        }
        Ok(Self(Sha256::digest(secret.as_bytes()).into()))
    }
    pub fn bytes(&self) -> &[u8] {
        &self.0
    }
    pub fn rate_key(&self) -> [u8; 32] {
        self.0
    }
}
pub fn generate() -> (String, TokenHash) {
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    let secret = format!("cbgpu_ct_{}", URL_SAFE_NO_PAD.encode(bytes));
    let hash = TokenHash(Sha256::digest(secret.as_bytes()).into());
    (secret, hash)
}
#[derive(Clone, Debug)]
pub struct Verified {
    pub subject: uuid::Uuid,
    pub session: uuid::Uuid,
}
pub async fn verify_jwt(identity: &dyn Identity, token: &str) -> Result<Verified> {
    if token.starts_with("cbgpu_ct_") {
        return Err(fail(
            401,
            "jwt_required",
            "Token management requires a real CBGPU access JWT",
        ));
    }
    let principal = identity.authenticate(token).await?;
    let parts: Vec<_> = token.split('.').collect();
    if parts.len() != 3 {
        return Err(fail(401, "jwt_required", "Use a CBGPU access JWT"));
    }
    let claims: Value = serde_json::from_slice(
        &URL_SAFE_NO_PAD
            .decode(parts[1])
            .map_err(|_| fail(401, "jwt_required", "Invalid access JWT"))?,
    )
    .map_err(|_| fail(401, "jwt_required", "Invalid access JWT"))?;
    if claims["sub"]
        .as_str()
        .and_then(|v| v.parse::<uuid::Uuid>().ok())
        != Some(principal.subject)
    {
        return Err(fail(401, "jwt_required", "Verified identity mismatch"));
    }
    let session = claims["session_id"]
        .as_str()
        .and_then(|v| v.parse().ok())
        .ok_or_else(|| fail(401, "jwt_required", "A current CBGPU session is required"))?;
    Ok(Verified {
        subject: principal.subject,
        session,
    })
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MintRequest {
    pub name: String,
    pub scopes: Vec<String>,
    pub allowed_contest_ids: Vec<uuid::Uuid>,
    #[serde(default = "default_days")]
    pub expires_in_days: u16,
}
fn default_days() -> u16 {
    30
}
impl MintRequest {
    pub fn validate(&self) -> Result<()> {
        if self.name.trim().is_empty()
            || self.name.trim().chars().count() > 80
            || self.name.contains('\0')
            || !(1..=90).contains(&self.expires_in_days)
            || self.allowed_contest_ids.is_empty()
            || self.allowed_contest_ids.len() > 100
            || self.scopes.is_empty()
            || self.scopes.len() > 3
            || self.scopes.iter().any(|s| {
                !["contests:read", "contests:register", "contests:cancel"].contains(&s.as_str())
            })
        {
            return Err(fail(
                400,
                "invalid_token_request",
                "Use a name, explicit read/register/cancel scopes,1–100 contest IDs and1–90 day expiry",
            ));
        }
        Ok(())
    }
}
#[async_trait::async_trait]
pub trait Persistence: Send + Sync {
    async fn mint(&self, actor: &Verified, hash: &TokenHash, input: &MintRequest) -> Result<Value>;
    async fn list(&self, actor: &Verified) -> Result<Value>;
    async fn revoke(&self, actor: &Verified, id: uuid::Uuid) -> Result<bool>;
    async fn read(&self, hash: &TokenHash, id: Option<uuid::Uuid>, own: bool) -> Result<Value>;
    async fn register(
        &self,
        hash: &TokenHash,
        id: uuid::Uuid,
        input: &RegistrationInput,
    ) -> Result<Value>;
    async fn cancel(&self, hash: &TokenHash, id: uuid::Uuid) -> Result<Value>;
}
