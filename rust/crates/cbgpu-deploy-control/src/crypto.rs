//! Node-compatible AES-256-GCM envelope. Existing keys and ciphertext are never regenerated on import.
use crate::{Result, fail};
use aes_gcm::{Aes256Gcm, KeyInit, aead::Aead};
use base64::{Engine, engine::general_purpose::STANDARD};
use rand::RngCore;
use serde_json::Value;
use subtle::ConstantTimeEq;
#[derive(Clone)]
pub struct Cipher {
    key: [u8; 32],
}
impl Cipher {
    pub fn new(key: &[u8]) -> Result<Self> {
        Ok(Self {
            key: key.try_into().map_err(|_| {
                fail(
                    "Existing deployment key must contain exactly32 raw bytes",
                    503,
                )
            })?,
        })
    }
    pub fn internal_key_matches(&self, value: &str) -> bool {
        let expected = self
            .key
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>();
        value.as_bytes().ct_eq(expected.as_bytes()).into()
    }
    pub fn seal(&self, value: &Value) -> Result<String> {
        let mut nonce = [0u8; 12];
        rand::thread_rng().fill_bytes(&mut nonce);
        self.seal_nonce(value, nonce)
    }
    fn seal_nonce(&self, value: &Value, nonce: [u8; 12]) -> Result<String> {
        let cipher = Aes256Gcm::new_from_slice(&self.key)
            .map_err(|_| fail("Invalid deployment encryption key", 503))?;
        let data = serde_json::to_vec(value)?;
        let ciphertext = cipher
            .encrypt(&nonce.into(), data.as_slice())
            .map_err(|_| fail("Cannot seal deployment secrets", 503))?;
        let mut result = nonce.to_vec();
        result.extend(ciphertext);
        Ok(STANDARD.encode(result))
    }
    pub fn unseal(&self, value: &str) -> Result<Value> {
        let bytes = STANDARD
            .decode(value)
            .map_err(|_| fail("Stored deployment secrets are invalid", 503))?;
        if bytes.len() < 28 {
            return Err(fail("Stored deployment secrets are truncated", 503));
        }
        let cipher = Aes256Gcm::new_from_slice(&self.key)
            .map_err(|_| fail("Invalid deployment encryption key", 503))?;
        let nonce: [u8; 12] = bytes[..12].try_into().unwrap();
        let plaintext = cipher
            .decrypt(&nonce.into(), &bytes[12..])
            .map_err(|_| fail("Deployment key does not authenticate stored secrets", 503))?;
        serde_json::from_slice(&plaintext)
            .map_err(|_| fail("Stored deployment secret payload is invalid", 503))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn ciphertext_authentication_and_api_key_are_exact() {
        let c = Cipher::new(&[7; 32]).unwrap();
        let v = json!({"env":{"TOKEN":"private"},"git_token":"secret"});
        let x = c.seal_nonce(&v, [3; 12]).unwrap();
        assert_eq!(
            x,
            "AwMDAwMDAwMDAwMDXtzGbSwKZDlYFAwXrhndNs9MkobC9OAVDaOrk4SMNOfFyBZH9dmfYr3tKHfousU4vvyMdbe+YoRrLZ8lRN49ew==",
            "matches actual Node crypto AES-256-GCM fixture"
        );
        assert_eq!(c.unseal(&x).unwrap(), v);
        assert!(Cipher::new(&[8; 32]).unwrap().unseal(&x).is_err());
        assert!(c.internal_key_matches(&"07".repeat(32)));
        assert!(!c.internal_key_matches("short"));
        assert!(!x.contains("private"));
    }
}
