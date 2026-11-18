use crate::{Error, Result};
use async_trait::async_trait;
use ed25519_dalek::VerifyingKey;
use serde_json::{Value, json};
#[derive(Clone)]
pub struct Identity {
    pub key: VerifyingKey,
    pub version: i64,
}
#[derive(Clone)]
pub struct Grant {
    pub job_id: String,
    pub node_id: String,
    pub version: i64,
    pub relay_session_id: String,
    pub expires_at: i64,
}
#[async_trait]
pub trait Authority: Send + Sync {
    async fn identity(&self, node: &str) -> Result<Identity>;
    async fn grant(&self, token: &str, job: &str) -> Result<Grant>;
    async fn relay(&self, node: &str, version: i64, connected: bool) -> Result<()>;
}
pub struct PostgresAuthority {
    pub backend: String,
    pub service_key: String,
    pub client: reqwest::Client,
}
impl PostgresAuthority {
    async fn rpc(&self, name: &str, value: Value) -> Result<Value> {
        let mut response = self
            .client
            .post(format!(
                "{}/rest/v1/rpc/{name}",
                self.backend.trim_end_matches('/')
            ))
            .header("apikey", &self.service_key)
            .bearer_auth(&self.service_key)
            .json(&value)
            .send()
            .await
            .map_err(|_| Error::Unavailable)?;
        let status = response.status();
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| Error::Unavailable)? {
            if bytes.len() + chunk.len() > 65536 {
                return Err(Error::Unavailable);
            }
            bytes.extend_from_slice(&chunk)
        }
        let value: Value = serde_json::from_slice(&bytes).map_err(|_| Error::Unavailable)?;
        if !status.is_success() {
            return Err(if value["message"].as_str() == Some("NODE_UNAVAILABLE") {
                Error::Denied
            } else {
                Error::Unavailable
            });
        }
        Ok(value)
    }
}
#[async_trait]
impl Authority for PostgresAuthority {
    async fn identity(&self, node: &str) -> Result<Identity> {
        let v = self
            .rpc("school_gpu_node_identity", json!({"p_node_id":node}))
            .await?;
        let key = cbgpu_notebook_protocol::unhex::<32>(
            v["public_key"].as_str().ok_or(Error::Unavailable)?,
        )
        .ok_or(Error::Unavailable)?;
        Ok(Identity {
            key: VerifyingKey::from_bytes(&key).map_err(|_| Error::Unavailable)?,
            version: v["credential_version"]
                .as_i64()
                .filter(|n| *n > 0)
                .ok_or(Error::Unavailable)?,
        })
    }
    async fn grant(&self, token: &str, job: &str) -> Result<Grant> {
        let v = self
            .rpc(
                "school_gpu_notebook_session",
                json!({"p_token":token,"p_job_id":job}),
            )
            .await?;
        if v["valid"] != true {
            return Err(Error::Denied);
        }
        let get = |name: &str| {
            v[name]
                .as_str()
                .map(str::to_owned)
                .ok_or(Error::Unavailable)
        };
        let g = Grant {
            job_id: get("job_id")?,
            node_id: get("node_id")?,
            version: v["credential_version"].as_i64().ok_or(Error::Unavailable)?,
            relay_session_id: get("relay_session_id")?,
            expires_at: chrono::DateTime::parse_from_rfc3339(&get("expires_at")?)
                .map_err(|_| Error::Unavailable)?
                .timestamp(),
        };
        if g.job_id != job
            || !cbgpu_notebook_protocol::uuid(&g.node_id)
            || !cbgpu_notebook_protocol::uuid(&g.relay_session_id)
            || g.expires_at <= crate::now()
        {
            return Err(Error::Denied);
        }
        Ok(g)
    }
    async fn relay(&self, node: &str, version: i64, connected: bool) -> Result<()> {
        let v = self
            .rpc(
                "school_gpu_notebook_relay",
                json!({"p_node_id":node,"p_version":version,"p_connected":connected}),
            )
            .await?;
        if v["ok"] == true {
            Ok(())
        } else {
            Err(Error::Denied)
        }
    }
}
