//! Native portal identity core candidate. No operations/terminal takeover or implicit handoff.
pub mod crypto;
use serde::{Deserialize, Serialize};
use uuid::Uuid;
#[derive(Debug)]
pub struct Error {
    pub status: u16,
    pub code: &'static str,
}
pub type Result<T> = std::result::Result<T, Error>;
pub fn fail(code: &'static str, status: u16) -> Error {
    Error { code, status }
}
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(i64::MAX as u128) as i64
}
pub fn flag(value: Option<&str>) -> Result<bool> {
    match value.unwrap_or("") {
        "" | "0" | "false" => Ok(false),
        "1" | "true" => Ok(true),
        _ => Err(fail("INVALID_SCOPE_FLAG", 503)),
    }
}
#[derive(Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct Member {
    pub id: Uuid,
    pub email: String,
}
#[derive(Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct Tokens {
    pub access_token: String,
    pub refresh_token: String,
}
#[derive(Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct SessionRow {
    pub token_hash: String,
    pub csrf: String,
    pub expires: i64,
    pub user_id: Uuid,
    pub email: String,
    pub auth_version: String,
    pub auth_session_id: Uuid,
    pub auth_tokens: String,
}
#[derive(Clone, Deserialize, Serialize)]
pub struct Principal {
    pub valid: bool,
    pub id: Uuid,
    pub email: String,
    pub name: String,
    pub is_admin: bool,
    pub legacy_approved: bool,
}
#[derive(Clone, Serialize)]
pub struct User {
    pub id: Uuid,
    pub email: String,
    pub name: String,
    pub verification_method: &'static str,
}
pub fn roster_member<'a>(roster: &'a [Member], email: &str) -> Option<&'a Member> {
    roster.iter().find(|m| m.email.eq_ignore_ascii_case(email))
}
pub fn validate_roster(roster: &[Member]) -> Result<()> {
    let mut ids = std::collections::HashSet::new();
    let mut emails = std::collections::HashSet::new();
    if roster.len() != 2 {
        return Err(fail("INVALID_DEVELOPER_ROSTER", 503));
    }
    for m in roster {
        if !ids.insert(m.id)
            || !emails.insert(m.email.to_lowercase())
            || m.email.len() > 254
            || !m.email.contains('@')
            || m.email.chars().any(char::is_whitespace)
        {
            return Err(fail("INVALID_DEVELOPER_ROSTER", 503));
        }
    }
    Ok(())
}
pub mod authority;
pub mod core;
pub mod http;
pub mod importer;
pub mod proxy_contract;
pub mod store;
