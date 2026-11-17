//! Fresh account authority. No browser-supplied role claims or privileged GoTrue key.
use crate::{Principal, Result, Tokens, fail};
use reqwest::Client;
use serde_json::{Value, json};
use sqlx::PgPool;
use std::time::Duration;
use url::Url;
use uuid::Uuid;
#[derive(Clone)]
pub struct GoTrue {
    client: Client,
    origin: Url,
    anon: String,
}
impl GoTrue {
    pub fn new(origin: &str, anon: String, private_container: bool) -> Result<Self> {
        let origin = Url::parse(origin).map_err(|_| fail("INVALID_AUTH_ORIGIN", 503))?;
        let host = origin.host_str().unwrap_or("");
        let local = matches!(host, "localhost" | "127.0.0.1" | "[::1]");
        let private = private_container
            && host.split('.').all(|s| {
                !s.is_empty()
                    && s.bytes()
                        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
            })
            && (!host.contains('.')
                || host.ends_with(".internal")
                || host.ends_with(".svc.cluster.local"));
        if !matches!(origin.scheme(), "http" | "https")
            || (!local && !private)
            || !origin.username().is_empty()
            || origin.password().is_some()
            || origin.query().is_some()
            || origin.fragment().is_some()
            || origin.path() != "/"
            || anon.is_empty()
        {
            return Err(fail("INVALID_AUTH_ORIGIN", 503));
        }
        let client = Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(5))
            .build()
            .map_err(|_| fail("AUTH_UNAVAILABLE", 503))?;
        Ok(Self {
            client,
            origin,
            anon,
        })
    }
    async fn request(&self, path: &str, token: Option<&str>, body: Option<Value>) -> Result<Value> {
        let url = self
            .origin
            .join(path)
            .map_err(|_| fail("AUTH_UNAVAILABLE", 503))?;
        let mut request = self
            .client
            .request(
                if body.is_some() {
                    reqwest::Method::POST
                } else {
                    reqwest::Method::GET
                },
                url,
            )
            .header("apikey", &self.anon);
        if let Some(token) = token {
            request = request.bearer_auth(token)
        }
        if let Some(body) = body {
            request = request.json(&body)
        }
        let mut response = request
            .send()
            .await
            .map_err(|_| fail("AUTH_UNAVAILABLE", 503))?;
        if !response.status().is_success() {
            return Err(if response.status().as_u16() == 429 {
                fail("TOO_MANY_ATTEMPTS", 429)
            } else if response.status().is_server_error() {
                fail("AUTH_UNAVAILABLE", 503)
            } else {
                fail("ACCOUNT_CREDENTIALS_OR_ACCESS_UNAVAILABLE", 401)
            });
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| fail("AUTH_UNAVAILABLE", 503))?
        {
            if bytes.len().saturating_add(chunk.len()) > 65536 {
                return Err(fail("AUTH_RESPONSE_INVALID", 503));
            }
            bytes.extend_from_slice(&chunk)
        }
        serde_json::from_slice(&bytes).map_err(|_| fail("AUTH_RESPONSE_INVALID", 503))
    }
    pub async fn login(&self, email: &str, password: &str) -> Result<(Uuid, Tokens)> {
        let value = self
            .request(
                "auth/v1/token?grant_type=password",
                None,
                Some(json!({"email":email,"password":password})),
            )
            .await?;
        let id = value["user"]["id"]
            .as_str()
            .and_then(|s| Uuid::parse_str(s).ok())
            .ok_or_else(|| fail("ACCOUNT_CREDENTIALS_OR_ACCESS_UNAVAILABLE", 401))?;
        let tokens = Tokens {
            access_token: value["access_token"]
                .as_str()
                .filter(|v| v.len() <= 8192)
                .ok_or_else(|| fail("AUTH_RESPONSE_INVALID", 503))?
                .into(),
            refresh_token: value["refresh_token"]
                .as_str()
                .filter(|v| !v.is_empty() && v.len() <= 4096)
                .ok_or_else(|| fail("AUTH_RESPONSE_INVALID", 503))?
                .into(),
        };
        let verified = self
            .request("auth/v1/user", Some(&tokens.access_token), None)
            .await?;
        if verified["id"] != id.to_string() {
            return Err(fail("ACCOUNT_IDENTITY_MISMATCH", 401));
        }
        let status = self
            .request(
                "rest/v1/rpc/cbgpu_self_session_status",
                Some(&tokens.access_token),
                Some(json!({})),
            )
            .await?;
        if status["valid"] != true {
            return Err(fail("ACCOUNT_SESSION_REVOKED", 401));
        }
        Ok((id, tokens))
    }
}
#[derive(Clone)]
pub struct Oracle {
    pool: PgPool,
}
impl Oracle {
    pub async fn connect(url: &str) -> Result<Self> {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(4)
            .acquire_timeout(Duration::from_secs(3))
            .after_connect(|c, _| {
                Box::pin(async move {
                    sqlx::query("SET ROLE cbgpu_portal_auth_reader")
                        .execute(&mut *c)
                        .await?;
                    sqlx::query("SET statement_timeout='3000ms'")
                        .execute(c)
                        .await?;
                    Ok(())
                })
            })
            .connect(url)
            .await
            .map_err(|_| fail("ROLE_AUTHORITY_UNAVAILABLE", 503))?;
        Ok(Self { pool })
    }
    pub fn from_pool(pool: PgPool) -> Self {
        Self { pool }
    }
    pub async fn principal(&self, id: Uuid, session: Uuid) -> Result<Option<Principal>> {
        let value: Value = sqlx::query_scalar("SELECT cbgpu_portal_auth.principal($1,$2)")
            .bind(id)
            .bind(session)
            .fetch_one(&self.pool)
            .await
            .map_err(|_| fail("ROLE_AUTHORITY_UNAVAILABLE", 503))?;
        if value["valid"] != true {
            return Ok(None);
        }
        let principal: Principal =
            serde_json::from_value(value).map_err(|_| fail("ROLE_AUTHORITY_INVALID", 503))?;
        if principal.id != id {
            return Err(fail("ROLE_AUTHORITY_INVALID", 503));
        }
        Ok(Some(principal))
    }
}
