use crate::{Identity, Principal, Result, fail};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde_json::Value;
#[async_trait::async_trait]
pub trait Sessions: Send + Sync {
    async fn active(&self, session: uuid::Uuid, subject: uuid::Uuid) -> Result<Option<bool>>;
}
pub struct PgSessions {
    pub pool: sqlx::PgPool,
}
#[async_trait::async_trait]
impl Sessions for PgSessions {
    async fn active(&self, session: uuid::Uuid, subject: uuid::Uuid) -> Result<Option<bool>> {
        let mut tx = self.pool.begin().await.map_err(|_| {
            fail(
                503,
                "authentication_unavailable",
                "Fresh session verification unavailable",
            )
        })?;
        let result = async {
            sqlx::query("SELECT set_config('request.jwt.claims',$1,true)")
                .bind(
                    serde_json::json!({"role":"authenticated","sub":subject,"session_id":session})
                        .to_string(),
                )
                .execute(&mut *tx)
                .await?;
            sqlx::query_as::<_, (bool, bool)>(
                "SELECT valid,approved FROM cbgpu_contests_auth.session_status($1,$2)",
            )
            .bind(subject)
            .bind(session)
            .fetch_optional(&mut *tx)
            .await
        }
        .await;
        let rollback = tx.rollback().await;
        let row = result.map_err(|_| {
            fail(
                503,
                "authentication_unavailable",
                "Fresh session verification unavailable",
            )
        })?;
        rollback.map_err(|_| {
            fail(
                503,
                "authentication_unavailable",
                "Fresh session verification unavailable",
            )
        })?;
        Ok(row.and_then(|(valid, approved)| valid.then_some(approved)))
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AuthorizationMode {
    pub account_v2: bool,
    pub multischool: bool,
}
pub fn authorization_mode(
    account: Option<&str>,
    school: Option<&str>,
) -> Result<AuthorizationMode> {
    fn flag(value: Option<&str>) -> Result<bool> {
        match value {
            None | Some("" | "0" | "false") => Ok(false),
            Some("1" | "true") => Ok(true),
            _ => Err(fail(
                503,
                "invalid_authorization_configuration",
                "Invalid contest authorization configuration",
            )),
        }
    }
    let multischool = flag(school)?;
    Ok(AuthorizationMode {
        account_v2: flag(account)? || multischool,
        multischool,
    })
}
pub fn authorization_mode_from_env() -> Result<AuthorizationMode> {
    fn value(key: &str) -> Result<Option<String>> {
        std::env::var_os(key)
            .map(|v| {
                v.into_string().map_err(|_| {
                    fail(
                        503,
                        "invalid_authorization_configuration",
                        "Invalid contest authorization configuration",
                    )
                })
            })
            .transpose()
    }
    authorization_mode(
        value("CBGPU_ACCOUNT_V2")?.as_deref(),
        value("CBGPU_MULTISCHOOL_V1")?.as_deref(),
    )
}

pub struct Auth {
    pub origin: String,
    pub anon_key: String,
    pub sessions: std::sync::Arc<dyn Sessions>,
    pub client: reqwest::Client,
    pub account_v2: bool,
    pub multischool: bool,
}
#[async_trait::async_trait]
impl Identity for Auth {
    async fn authenticate(&self, token: &str) -> Result<Principal> {
        let claims = claims(token)?;
        let subject = claims["sub"]
            .as_str()
            .and_then(|v| uuid::Uuid::parse_str(v).ok())
            .ok_or_else(|| fail(401, "unauthorized", "Access JWT subject missing"))?;
        let session = claims["session_id"]
            .as_str()
            .and_then(|v| uuid::Uuid::parse_str(v).ok())
            .ok_or_else(|| fail(401, "unauthorized", "Use a current signed-in CBGPU session"))?;
        if claims["role"] != "authenticated"
            || claims["exp"]
                .as_f64()
                .is_none_or(|exp| !exp.is_finite() || exp <= now())
        {
            return Err(fail(
                401,
                "unauthorized",
                "Access JWT expired or has the wrong role",
            ));
        }
        let response = self
            .client
            .get(format!("{}/auth/v1/user", self.origin))
            .header("apikey", &self.anon_key)
            .bearer_auth(token)
            .send()
            .await
            .map_err(|_| {
                fail(
                    503,
                    "authentication_unavailable",
                    "CBGPU authentication unavailable",
                )
            })?;
        if !response.status().is_success() {
            return Err(fail(
                401,
                "unauthorized",
                "CBGPU access JWT was not accepted",
            ));
        }
        let user: Value = response.json().await.map_err(|_| {
            fail(
                503,
                "authentication_unavailable",
                "Invalid authentication response",
            )
        })?;
        if user["id"]
            .as_str()
            .and_then(|v| uuid::Uuid::parse_str(v).ok())
            != Some(subject)
        {
            return Err(fail(
                401,
                "unauthorized",
                "Verified account does not match JWT subject",
            ));
        }
        let legacy_permission = self
            .sessions
            .active(session, subject)
            .await?
            .ok_or_else(|| {
                fail(
                    401,
                    "session_revoked",
                    "Session expired, signed out, or account is unavailable",
                )
            })?;
        // School resource decisions require the actual contest, not the user's
        // primary affiliation. Reads/cancellation remain available after expiry.
        let may_register = self.account_v2 || legacy_permission;
        Ok(Principal {
            subject,
            may_register,
        })
    }
    async fn may_register_contest(
        &self,
        token: &str,
        contest: uuid::Uuid,
        principal: &Principal,
    ) -> Result<bool> {
        if !self.account_v2 {
            return Ok(principal.may_register);
        }
        let multischool = self.multischool;
        let (rpc, payload) = if multischool {
            (
                "cbgpu_can_register_contest",
                serde_json::json!({"p_contest":contest}),
            )
        } else {
            (
                "cbgpu_can_use_resource",
                serde_json::json!({"resource":"contests","target_school":"cbsh"}),
            )
        };
        let response = self
            .client
            .post(format!("{}/rest/v1/rpc/{rpc}", self.origin))
            .header("apikey", &self.anon_key)
            .bearer_auth(token)
            .json(&payload)
            .send()
            .await
            .map_err(|_| {
                fail(
                    503,
                    "authorization_unavailable",
                    "Contest permission unavailable",
                )
            })?;
        if !response.status().is_success() {
            return Err(fail(
                503,
                "authorization_unavailable",
                "Contest permission unavailable",
            ));
        }
        let allowed: bool = response.json().await.map_err(|_| {
            fail(
                503,
                "authorization_unavailable",
                "Contest permission unavailable",
            )
        })?;
        Ok(allowed)
    }
}
fn now() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}
fn claims(token: &str) -> Result<Value> {
    let pieces: Vec<_> = token.split('.').collect();
    if token.len() > 16384 || pieces.len() != 3 {
        return Err(fail(401, "unauthorized", "Use a CBGPU access JWT"));
    }
    let bytes = URL_SAFE_NO_PAD
        .decode(pieces[1])
        .map_err(|_| fail(401, "unauthorized", "Invalid access JWT"))?;
    serde_json::from_slice(&bytes).map_err(|_| fail(401, "unauthorized", "Invalid access JWT"))
}

#[cfg(test)]
mod mode_tests {
    use super::*;
    #[test]
    fn multischool_always_enforces_contest_specific_policy() {
        for active in ["1", "true"] {
            let mode = authorization_mode(Some("0"), Some(active)).unwrap();
            assert!(mode.account_v2 && mode.multischool);
        }
        assert_eq!(
            authorization_mode(None, None).unwrap(),
            AuthorizationMode {
                account_v2: false,
                multischool: false
            }
        );
        assert_eq!(
            authorization_mode(Some("true"), None).unwrap(),
            AuthorizationMode {
                account_v2: true,
                multischool: false
            }
        );
        for bad in ["TRUE", "yes", "1 ", "falsex"] {
            assert!(authorization_mode(Some(bad), None).is_err());
            assert!(authorization_mode(None, Some(bad)).is_err());
        }
    }
}
