//! The actual regular-site shared session cookie is readable JSON chunks, not an opaque ticket.
//! Only access_token is consumed. Claims never determine identity or booking ownership.
use crate::{Error, Result, now};
use axum::http::HeaderMap;
use base64::Engine;
use serde_json::{Value, json};
/// Canonical server flags only; validate both even when one already enables scope.
pub fn authorization_mode(account: Option<&str>, schools: Option<&str>) -> Result<bool> {
    let mut enabled = false;
    for value in [account, schools].into_iter().flatten() {
        match value {
            "" | "0" | "false" => {}
            "1" | "true" => enabled = true,
            _ => {
                return Err(Error {
                    status: 503,
                    message: "Invalid GPU authorization configuration".into(),
                });
            }
        }
    }
    Ok(enabled)
}
pub fn authorization_from_environment() -> Result<bool> {
    let values = ["CBGPU_ACCOUNT_V2", "CBGPU_MULTISCHOOL_V1"]
        .into_iter()
        .map(|key| match std::env::var(key) {
            Ok(value) => Ok(Some(value)),
            Err(std::env::VarError::NotPresent) => Ok(None),
            Err(_) => Err(Error::unavailable()),
        })
        .collect::<Result<Vec<_>>>()?;
    authorization_mode(values[0].as_deref(), values[1].as_deref())
}
#[derive(Clone, Debug)]
pub struct Grant {
    pub owner: String,
    pub ends_at: f64,
}
#[async_trait::async_trait]
pub trait Authority: Send + Sync {
    async fn authorize(&self, token: &str, gpu: u8) -> Result<Grant>;
}
#[derive(Clone)]
pub struct SupabaseAuthority {
    pub origin: String,
    pub anon_key: String,
    pub service_key: String,
    pub account_v2: bool,
    pub client: reqwest::Client,
}
fn deny(message: &str) -> Error {
    Error {
        status: 403,
        message: message.into(),
    }
}
#[async_trait::async_trait]
impl Authority for SupabaseAuthority {
    async fn authorize(&self, token: &str, gpu: u8) -> Result<Grant> {
        if !(1..=4).contains(&gpu) {
            return Err(deny("Invalid GPU"));
        }
        let response = self
            .client
            .get(format!("{}/auth/v1/user", self.origin))
            .header("apikey", &self.anon_key)
            .bearer_auth(token)
            .send()
            .await
            .map_err(|_| Error::unavailable())?;
        if !response.status().is_success() {
            return Err(Error {
                status: 401,
                message: "CBGPU session expired; sign in again".into(),
            });
        }
        let user: Value = response.json().await.map_err(|_| Error::unavailable())?;
        let user = user.get("user").filter(|v| v.is_object()).unwrap_or(&user);
        let owner = user["id"]
            .as_str()
            .filter(|s| uuid::Uuid::parse_str(s).is_ok())
            .ok_or_else(|| deny("Invalid CBGPU identity"))?;
        if self.account_v2 {
            let response = self
                .client
                .post(format!("{}/rest/v1/rpc/cbgpu_notebook_grant", self.origin))
                .header("apikey", &self.anon_key)
                .bearer_auth(token)
                .json(&json!({"p_gpu":gpu}))
                .send()
                .await
                .map_err(|_| Error::unavailable())?;
            if !response.status().is_success() {
                return Err(if response.status().as_u16() == 403 {
                    deny("Session, school scope or notebook ownership was revoked")
                } else {
                    Error::unavailable()
                });
            }
            let grant: Value = response.json().await.map_err(|_| Error::unavailable())?;
            let end = grant["ends_at"]
                .as_f64()
                .filter(|v| v.is_finite() && *v > now())
                .ok_or_else(|| deny("Notebook booking expired"))?;
            if grant["owner"] != owner {
                return Err(deny("Notebook grant identity mismatch"));
            }
            return Ok(Grant {
                owner: owner.into(),
                ends_at: end,
            });
        } else {
            let response = self
                .client
                .get(format!("{}/auth/v1/admin/users/{owner}", self.origin))
                .header("apikey", &self.service_key)
                .bearer_auth(&self.service_key)
                .send()
                .await
                .map_err(|_| Error::unavailable())?;
            if !response.status().is_success() {
                return Err(Error::unavailable());
            }
            let current: Value = response.json().await.map_err(|_| Error::unavailable())?;
            let current = current
                .get("user")
                .filter(|v| v.is_object())
                .unwrap_or(&current);
            if current["id"] != owner || current.get("deleted_at").is_some_and(|v| !v.is_null()) {
                return Err(deny("Account is unavailable"));
            }
            if let Some(until) = current["banned_until"].as_str().filter(|s| !s.is_empty())
                && parse_time(until).ok_or_else(Error::unavailable)? > now()
            {
                return Err(deny("Account is suspended"));
            }
            let profile = self
                .client
                .get(format!("{}/rest/v1/profiles", self.origin))
                .header("apikey", &self.anon_key)
                .bearer_auth(token)
                .query(&[
                    ("id", format!("eq.{owner}")),
                    ("select", "id,is_approved".into()),
                ])
                .send()
                .await
                .map_err(|_| Error::unavailable())?;
            if !profile.status().is_success() {
                return Err(Error::unavailable());
            }
            let rows: Value = profile.json().await.map_err(|_| Error::unavailable())?;
            if !rows.as_array().is_some_and(|a| {
                a.len() == 1 && a[0]["id"] == owner && a[0]["is_approved"] != false
            }) {
                return Err(deny("Current school GPU permission is required"));
            }
        }
        // Query with the caller's JWT so existing ownership RLS remains effective.
        // Explicit user_id prevents any admin SELECT policy from becoming notebook access.
        let at = chrono::Utc::now().to_rfc3339();
        let response = self
            .client
            .get(format!("{}/rest/v1/reservations", self.origin))
            .header("apikey", &self.anon_key)
            .bearer_auth(token)
            .query(&[
                (
                    "select",
                    "id,user_id,gpu_id,status,start_time,end_time".to_owned(),
                ),
                ("user_id", format!("eq.{owner}")),
                ("gpu_id", format!("eq.{gpu}")),
                ("status", "eq.approved".into()),
                ("start_time", format!("lte.{at}")),
                ("end_time", format!("gt.{at}")),
                ("order", "end_time.desc".into()),
                ("limit", "1".into()),
            ])
            .send()
            .await
            .map_err(|_| Error::unavailable())?;
        if !response.status().is_success() {
            return Err(Error::unavailable());
        }
        let rows: Value = response.json().await.map_err(|_| Error::unavailable())?;
        let reservation = rows
            .as_array()
            .and_then(|a| a.first())
            .ok_or_else(|| deny("An active approved reservation for this GPU is required"))?;
        let start = reservation["start_time"]
            .as_str()
            .and_then(parse_time)
            .ok_or_else(Error::unavailable)?;
        let end = reservation["end_time"]
            .as_str()
            .and_then(parse_time)
            .ok_or_else(Error::unavailable)?;
        if reservation["user_id"] != owner
            || reservation["gpu_id"].as_u64() != Some(gpu as u64)
            || reservation["status"] != "approved"
            || start > now()
            || end <= now()
        {
            return Err(deny("Reservation ownership or validity changed"));
        }
        Ok(Grant {
            owner: owner.into(),
            ends_at: end,
        })
    }
}
pub fn parse_time(value: &str) -> Option<f64> {
    chrono::DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|t| t.timestamp_millis() as f64 / 1000.)
}
pub fn credential(headers: &HeaderMap, key: &str) -> Result<String> {
    for header in headers.get_all("cookie") {
        if let Ok(raw) = header.to_str() {
            for item in raw.split(';') {
                if let Some((name, value)) = item.trim().split_once('=')
                    && name == "cbgpu-shared-session-state"
                    && percent_encoding::percent_decode_str(value).decode_utf8_lossy()
                        == "signed-out"
                {
                    return Err(Error {
                        status: 401,
                        message: "CBGPU session was signed out".into(),
                    });
                }
            }
        }
    }
    if let Some(value) = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
    {
        if value.is_empty() || value.len() > 16000 {
            return Err(deny("Invalid bearer token"));
        }
        return Ok(value.into());
    }
    let mut cookies = std::collections::HashMap::new();
    for header in headers.get_all("cookie") {
        let header = header.to_str().map_err(|_| deny("Invalid cookies"))?;
        if header.len() > 65536 {
            return Err(deny("Shared session cookie is too large"));
        }
        for part in header.split(';') {
            if let Some((name, value)) = part.trim().split_once('=') {
                let name = name.trim();
                if cookies
                    .insert(name.to_owned(), value.trim().to_owned())
                    .is_some()
                    && name.starts_with(key)
                {
                    return Err(deny("Ambiguous session cookies"));
                }
            }
        }
    }
    if cookies.keys().any(|name| {
        name.strip_prefix(&format!("{key}."))
            .and_then(|suffix| suffix.parse::<usize>().ok())
            .is_some_and(|index| index >= 32)
    }) {
        return Err(deny("Too many shared session chunks"));
    }
    let mut json = String::new();
    for i in 0..32 {
        let name = format!("{key}.{i}");
        let Some(chunk) = cookies.get(&name) else {
            if cookies.keys().any(|n| {
                n.strip_prefix(&format!("{key}."))
                    .and_then(|n| n.parse::<usize>().ok())
                    .is_some_and(|n| n > i)
            }) {
                return Err(deny("Incomplete shared session cookie"));
            }
            break;
        };
        json.push_str(chunk);
        if json.len() > 65536 {
            return Err(deny("Shared session cookie is too large"));
        }
    }
    if json.is_empty() {
        return Err(Error {
            status: 401,
            message: "Sign in to CBGPU before opening a notebook".into(),
        });
    }
    let json = percent_encoding::percent_decode_str(&json)
        .decode_utf8()
        .map_err(|_| deny("Invalid shared session encoding"))?;
    let session: Value = serde_json::from_str(&json).map_err(|_| deny("Invalid shared session"))?;
    session["access_token"]
        .as_str()
        .filter(|s| !s.is_empty() && s.len() <= 16000)
        .map(str::to_owned)
        .ok_or_else(|| deny("Shared session has no access token"))
}
pub fn token_expiry(token: &str) -> Option<f64> {
    let payload = token.split('.').nth(1)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .ok()?;
    let value: Value = serde_json::from_slice(&bytes).ok()?;
    value["exp"].as_f64()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn actual_shared_cookie_chunks_reconstruct_without_trusting_user_metadata() {
        let session=json!({"access_token":"valid-token","refresh_token":"private-refresh","user":{"id":"forged-metadata","name":"한글🙂"}}).to_string();
        let encoded =
            percent_encoding::utf8_percent_encode(&session, percent_encoding::NON_ALPHANUMERIC)
                .to_string();
        let middle = encoded.len() / 2;
        let mut headers = HeaderMap::new();
        headers.insert(
            "cookie",
            format!(
                "sb-cbgpu-auth-token.0={}; sb-cbgpu-auth-token.1={}",
                &encoded[..middle],
                &encoded[middle..]
            )
            .parse()
            .unwrap(),
        );
        assert_eq!(
            credential(&headers, "sb-cbgpu-auth-token").unwrap(),
            "valid-token"
        );
    }
    #[test]
    fn incomplete_duplicate_or_untrusted_cookie_is_denied() {
        for value in [
            "sb-cbgpu-auth-token.1=missingzero",
            "sb-cbgpu-auth-token.0=%7B; sb-cbgpu-auth-token.2=%7D",
            "sb-cbgpu-auth-token.0=x; sb-cbgpu-auth-token.0=y",
        ] {
            let mut headers = HeaderMap::new();
            headers.insert("cookie", value.parse().unwrap());
            assert!(credential(&headers, "sb-cbgpu-auth-token").is_err());
        }
    }
}
#[cfg(test)]
mod tombstone_tests {
    use super::*;
    #[test]
    fn signed_out_marker_wins_over_residual_chunks_and_bearer() {
        let value = percent_encoding::utf8_percent_encode(
            &json!({"access_token":"previous-session"}).to_string(),
            percent_encoding::NON_ALPHANUMERIC,
        )
        .to_string();
        let mut h = HeaderMap::new();
        h.insert("authorization", "Bearer previous-session".parse().unwrap());
        h.insert("cookie",format!("cbgpu-shared-session-state=active; sb-cbgpu-auth-token.0={value}; cbgpu-shared-session-state=signed-out").parse().unwrap());
        assert_eq!(
            credential(&h, "sb-cbgpu-auth-token").unwrap_err().status,
            401
        );
    }
}

#[cfg(test)]
mod authorization_flag_tests {
    use super::authorization_mode as mode;
    #[test]
    fn all_server_flag_pairs_are_validated() {
        for account in [
            None,
            Some(""),
            Some("0"),
            Some("false"),
            Some("1"),
            Some("true"),
        ] {
            for schools in [
                None,
                Some(""),
                Some("0"),
                Some("false"),
                Some("1"),
                Some("true"),
            ] {
                assert_eq!(
                    mode(account, schools).unwrap(),
                    matches!(account, Some("1" | "true")) || matches!(schools, Some("1" | "true"))
                );
            }
        }
        for bad in ["TRUE", "FALSE", " 0", "1 ", "yes", "tru"] {
            assert!(mode(Some("1"), Some(bad)).is_err());
            assert!(mode(Some(bad), Some("1")).is_err());
        }
    }
}
