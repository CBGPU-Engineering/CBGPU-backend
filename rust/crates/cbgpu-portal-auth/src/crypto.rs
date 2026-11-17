//! Exact Node developer/auth.mts envelope/hash compatibility; keys are never regenerated.
use crate::{Error, Result, SessionRow, Tokens, fail};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use cbgpu_deploy_control::crypto::Cipher;
use hmac::{Hmac, Mac};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use uuid::Uuid;
pub const COOKIE: &str = "__Host-cbgpu-developer";
pub struct Crypto {
    secret: Vec<u8>,
    cipher: Cipher,
}
#[derive(Deserialize)]
pub struct Claims {
    pub sub: Uuid,
    pub session_id: Uuid,
    pub exp: i64,
}
impl Crypto {
    pub fn new(secret: &str) -> Result<Self> {
        if secret.len() < 32 || secret.len() > 4096 {
            return Err(fail("INVALID_EXISTING_SESSION_KEY", 503));
        }
        let key = Sha256::digest(format!("portal-account-tokens:developer:{secret}"));
        Ok(Self {
            secret: secret.as_bytes().to_vec(),
            cipher: Cipher::new(&key).map_err(|_| fail("INVALID_EXISTING_SESSION_KEY", 503))?,
        })
    }
    pub fn hash(&self, value: &str) -> String {
        let mut mac =
            Hmac::<Sha256>::new_from_slice(&self.secret).expect("HMAC accepts existing key");
        mac.update(b"developer:");
        mac.update(value.as_bytes());
        mac.finalize()
            .into_bytes()
            .iter()
            .map(|v| format!("{v:02x}"))
            .collect()
    }
    pub fn version(&self, id: Uuid) -> String {
        self.hash(&format!("account-only-v1:{id}"))
    }
    pub fn seal(&self, tokens: &Tokens) -> Result<String> {
        self.cipher
            .seal(&serde_json::to_value(tokens).map_err(|_| fail("TOKEN_SEAL_FAILED", 503))?)
            .map_err(|_| fail("TOKEN_SEAL_FAILED", 503))
    }
    pub fn open(&self, sealed: &str) -> Result<Tokens> {
        if sealed.len() > 16384 {
            return Err(fail("STORED_TOKENS_INVALID", 401));
        }
        let value = self
            .cipher
            .unseal(sealed)
            .map_err(|_| fail("STORED_TOKENS_INVALID", 401))?;
        let tokens: Tokens =
            serde_json::from_value(value).map_err(|_| fail("STORED_TOKENS_INVALID", 401))?;
        if tokens.access_token.len() > 8192
            || tokens.refresh_token.is_empty()
            || tokens.refresh_token.len() > 4096
        {
            return Err(fail("STORED_TOKENS_INVALID", 401));
        }
        Ok(tokens)
    }
    pub fn row_tokens(&self, row: &SessionRow) -> Result<(Tokens, Claims)> {
        let tokens = self.open(&row.auth_tokens)?;
        let claims = claims(&tokens.access_token)?;
        if claims.sub != row.user_id
            || claims.session_id != row.auth_session_id
            || !same(&row.auth_version, &self.version(row.user_id))
        {
            return Err(fail("STORED_IDENTITY_MISMATCH", 401));
        }
        Ok((tokens, claims))
    }
}
/// Only call after GoTrue verification or after authenticating the AES envelope.
/// Decoding a caller-provided JWT is never an authorization decision.
pub fn claims(jwt: &str) -> Result<Claims> {
    if jwt.len() > 8192 {
        return Err(fail("INVALID_ACCOUNT_SESSION", 401));
    }
    let parts = jwt.split('.').collect::<Vec<_>>();
    if parts.len() != 3 {
        return Err(fail("INVALID_ACCOUNT_SESSION", 401));
    }
    let bytes = URL_SAFE_NO_PAD
        .decode(parts[1])
        .map_err(|_| fail("INVALID_ACCOUNT_SESSION", 401))?;
    serde_json::from_slice(&bytes).map_err(|_| fail("INVALID_ACCOUNT_SESSION", 401))
}
pub fn same(a: &str, b: &str) -> bool {
    a.len() == b.len() && bool::from(a.as_bytes().ct_eq(b.as_bytes()))
}
pub fn token(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
pub fn read_cookie(value: &str) -> Option<&str> {
    let values = value
        .split(';')
        .filter_map(|part| part.trim().strip_prefix(&format!("{COOKIE}=")))
        .collect::<Vec<_>>();
    if values.len() == 1 && token(values[0]) {
        Some(values[0])
    } else {
        None
    }
}
pub fn cookie(name: &str, value: &str, seconds: i64) -> String {
    format!("{name}={value}; Path=/; HttpOnly; Secure; SameSite=Strict; Max-Age={seconds}")
}
pub fn csrf(expected: &str, actual: &str) -> std::result::Result<(), Error> {
    if !token(expected) || !same(expected, actual) {
        Err(fail("INVALID_CSRF_TOKEN", 403))
    } else {
        Ok(())
    }
}
