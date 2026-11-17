//! Existing local Auth + authoritative server project ownership; never trusts resource-owner headers.
use crate::{Result, fail, valid_id};
use serde_json::{Value, json};
#[async_trait::async_trait]
pub trait Authority: Send + Sync {
    fn requires_request_actor(&self) -> bool {
        false
    }
    async fn identity(&self, user: &str) -> Result<()>;
    async fn can_execute(&self, user: &str) -> Result<bool>;
    async fn server_owner(&self, project: &str) -> Result<String>;
    async fn server_projects(&self) -> Result<Vec<Value>> {
        Err(fail("Compose supervision backend unavailable", 503))
    }
    async fn server_project(&self, id: &str) -> Result<Value> {
        self.server_projects()
            .await?
            .into_iter()
            .find(|p| p["id"] == id)
            .ok_or_else(|| fail("Project not found", 404))
    }
    async fn mark_deployed(&self, _id: &str, _time: Option<&str>) -> Result<()> {
        Err(fail("Server synchronization unavailable", 503))
    }
    async fn txt_ownership(&self, hostname: &str, expected: &str) -> Result<bool>;
}
pub struct BackendAuthority {
    pub origin: String,
    pub key: String,
    pub account_v2: bool,
    pub client: reqwest::Client,
}
impl BackendAuthority {
    async fn get(&self, path: &str) -> Result<Value> {
        let response = self
            .client
            .get(format!("{}{}", self.origin, path))
            .header("apikey", &self.key)
            .bearer_auth(&self.key)
            .send()
            .await
            .map_err(|_| fail("Account database unavailable", 503))?;
        if !response.status().is_success() {
            return Err(fail("Account database unavailable", 503));
        }
        response
            .json()
            .await
            .map_err(|_| fail("Invalid account database response", 503))
    }
}
#[async_trait::async_trait]
impl Authority for BackendAuthority {
    fn requires_request_actor(&self) -> bool {
        self.account_v2
    }
    async fn identity(&self, user: &str) -> Result<()> {
        if !valid_id(user) {
            return Err(fail("Forbidden", 403));
        }
        let raw = self.get(&format!("/auth/v1/admin/users/{user}")).await?;
        let account = raw.get("user").filter(|v| v.is_object()).unwrap_or(&raw);
        if account["id"] != user || account.get("deleted_at").is_some_and(|v| !v.is_null()) {
            return Err(fail("Account unavailable", 403));
        }
        if let Some(until) = account["banned_until"].as_str().filter(|v| !v.is_empty()) {
            let until = chrono::DateTime::parse_from_rfc3339(until)
                .map_err(|_| fail("Invalid account status", 503))?;
            if until > chrono::Utc::now() {
                return Err(fail("Account suspended", 403));
            }
        }
        if self.account_v2 {
            let rows = self
                .get(&format!(
                    "/rest/v1/profiles?id=eq.{user}&select=id,account_status"
                ))
                .await?;
            let profile = rows
                .as_array()
                .filter(|rows| rows.len() == 1)
                .and_then(|rows| rows.first());
            if !profile.is_some_and(|p| {
                p["id"] == user
                    && matches!(
                        p["account_status"].as_str(),
                        Some("ACTIVE" | "PENDING_VERIFICATION" | "REVERIFICATION_REQUIRED")
                    )
            }) {
                return Err(fail("Account unavailable", 403));
            }
        }
        Ok(())
    }
    async fn can_execute(&self, user: &str) -> Result<bool> {
        self.identity(user).await?;
        if self.account_v2 {
            let response = self
                .client
                .post(format!(
                    "{}/rest/v1/rpc/cbgpu_service_can_use_resource",
                    self.origin
                ))
                .header("apikey", &self.key)
                .bearer_auth(&self.key)
                .json(&json!({"user_id":user,"resource":"school_compute","target_school":"cbsh"}))
                .send()
                .await
                .map_err(|_| fail("School permission unavailable", 503))?;
            if !response.status().is_success() {
                return Err(fail("School permission unavailable", 503));
            }
            Ok(response
                .json::<Value>()
                .await
                .map_err(|_| fail("School permission unavailable", 503))?
                == true)
        } else {
            let rows = self
                .get(&format!(
                    "/rest/v1/profiles?id=eq.{user}&select=id,is_approved"
                ))
                .await?;
            Ok(rows.as_array().is_some_and(|v| {
                v.len() == 1 && v[0]["id"] == user && v[0]["is_approved"] != false
            }))
        }
    }
    async fn server_owner(&self, id: &str) -> Result<String> {
        if !valid_id(id) {
            return Err(fail("Project not found", 404));
        }
        let rows = self
            .get(&format!(
                "/rest/v1/cpu_projects?id=eq.{id}&select=id,user_id,status"
            ))
            .await?;
        let row = rows
            .as_array()
            .filter(|v| v.len() == 1)
            .and_then(|v| v.first())
            .filter(|v| v["id"] == id && v["status"] != "deleted")
            .ok_or_else(|| fail("Project not found", 404))?;
        row["user_id"]
            .as_str()
            .filter(|v| valid_id(v))
            .map(str::to_owned)
            .ok_or_else(|| fail("Invalid project ownership", 503))
    }
    async fn server_projects(&self) -> Result<Vec<Value>> {
        let value = self
            .get("/rest/v1/cpu_projects?select=*&status=in.(approved,deleted)")
            .await?;
        serde_json::from_value(value).map_err(|_| fail("Invalid server project list", 503))
    }
    async fn server_project(&self, id: &str) -> Result<Value> {
        if !valid_id(id) {
            return Err(fail("Project not found", 404));
        }
        let rows = self
            .get(&format!("/rest/v1/cpu_projects?id=eq.{id}&select=*"))
            .await?;
        rows.as_array()
            .filter(|v| v.len() == 1)
            .and_then(|v| v.first())
            .filter(|v| v["id"] == id)
            .cloned()
            .ok_or_else(|| fail("Project not found", 404))
    }
    async fn mark_deployed(&self, id: &str, time: Option<&str>) -> Result<()> {
        if !valid_id(id) {
            return Err(fail("Invalid project", 400));
        }
        let response = self
            .client
            .patch(format!("{}/rest/v1/cpu_projects?id=eq.{id}", self.origin))
            .header("apikey", &self.key)
            .bearer_auth(&self.key)
            .json(&json!({"deployed_at":time}))
            .send()
            .await
            .map_err(|_| fail("Server synchronization unavailable", 503))?;
        if !response.status().is_success() {
            return Err(fail("Server synchronization unavailable", 503));
        }
        Ok(())
    }
    async fn txt_ownership(&self, hostname: &str, expected: &str) -> Result<bool> {
        let name = format!("_cbgpu-verification.{hostname}");
        let response = self
            .client
            .get("https://cloudflare-dns.com/dns-query")
            .query(&[("name", name.as_str()), ("type", "TXT")])
            .header("accept", "application/dns-json")
            .send()
            .await
            .map_err(|_| fail("DNS verification unavailable", 503))?;
        if !response.status().is_success() {
            return Err(fail("DNS verification unavailable", 503));
        }
        let value: Value = response
            .json()
            .await
            .map_err(|_| fail("Invalid DNS verification response", 503))?;
        Ok(value["Answer"].as_array().is_some_and(|rows| {
            rows.iter().any(|row| {
                row["type"] == 16
                    && row["name"]
                        .as_str()
                        .is_some_and(|v| v.trim_end_matches('.').eq_ignore_ascii_case(&name))
                    && row["data"]
                        .as_str()
                        .and_then(|v| serde_json::from_str::<String>(v).ok())
                        .as_deref()
                        == Some(expected)
            })
        }))
    }
}

#[cfg(test)]
mod identity_tests {
    use super::*;
    #[tokio::test]
    async fn fresh_profile_suspension_blocks_identity_without_erasing_project_rights() {
        let id = "11111111-1111-4111-8111-111111111111";
        for status in [
            "ACTIVE",
            "PENDING_VERIFICATION",
            "REVERIFICATION_REQUIRED",
            "SUSPENDED",
            "UNKNOWN",
        ] {
            let app = axum::Router::new().fallback(move |uri: axum::http::Uri| async move {
                axum::Json(if uri.path().starts_with("/auth/") {
                    json!({"id":id,"deleted_at":null,"banned_until":null})
                } else {
                    json!([{"id":id,"account_status":status,"is_approved":false}])
                })
            });
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
            let authority = BackendAuthority {
                origin: format!("http://{address}"),
                key: "synthetic-only".into(),
                account_v2: true,
                client: reqwest::Client::new(),
            };
            assert_eq!(
                authority.identity(id).await.is_ok(),
                ["ACTIVE", "PENDING_VERIFICATION", "REVERIFICATION_REQUIRED"].contains(&status),
                "{status}"
            );
            task.abort();
        }
    }
}
