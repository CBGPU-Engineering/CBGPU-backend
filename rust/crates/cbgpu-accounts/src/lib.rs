//! Account API adapter: credentials and authorization remain in existing Auth/PostgreSQL.
//! Every request forwards its current bearer token to a guarded SQL RPC; no cached roles.
use axum::{
    Json, Router,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    routing::{get, patch, post},
};
use serde_json::{Value, json};
use std::sync::Arc;

pub struct Config {
    pub backend_url: String,
    pub anon_key: String,
    pub client: reqwest::Client,
}
fn token(headers: &HeaderMap) -> Result<&str, (StatusCode, Json<Value>)> {
    (headers.get_all("authorization").iter().count() == 1)
        .then(|| headers.get("authorization"))
        .flatten()
        .and_then(|v| v.to_str().ok())
        .filter(|s| {
            s.split_once(' ').is_some_and(|(scheme, value)| {
                scheme.eq_ignore_ascii_case("Bearer")
                    && !value.is_empty()
                    && !value.bytes().any(|b| b.is_ascii_whitespace())
            })
        })
        .ok_or((
            StatusCode::UNAUTHORIZED,
            Json(json!({"error":"Authentication required"})),
        ))
}
async fn call(
    config: Arc<Config>,
    headers: HeaderMap,
    action: &str,
    payload: Value,
) -> (StatusCode, Json<Value>) {
    let bearer = match token(&headers) {
        Ok(v) => v,
        Err(e) => return e,
    };
    // Auth getUser verifies the session with the existing service, rather than decoding
    // user-controlled metadata or using a service-role identity for the caller.
    let verify = config
        .client
        .get(format!("{}/auth/v1/user", config.backend_url))
        .header("apikey", &config.anon_key)
        .header("authorization", bearer)
        .send()
        .await;
    match verify {
        Ok(r) if r.status().is_success() => (),
        Ok(_) => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(json!({"error":"Session expired"})),
            );
        }
        Err(_) => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({"error":"Authentication unavailable"})),
            );
        }
    };
    let response = config
        .client
        .post(format!(
            "{}/rest/v1/rpc/cbgpu_account_api",
            config.backend_url
        ))
        .header("apikey", &config.anon_key)
        .header("authorization", bearer)
        .json(&json!({"action":action,"payload":payload}))
        .send()
        .await;
    match response {
        Ok(r) => {
            let status = r.status();
            let body = r
                .json::<Value>()
                .await
                .unwrap_or(json!({"error":"Invalid backend response"}));
            if status.is_success() {
                (
                    if body.get("ok") == Some(&Value::Bool(false)) {
                        StatusCode::BAD_REQUEST
                    } else {
                        StatusCode::OK
                    },
                    Json(body),
                )
            } else {
                // SQL exception messages are deliberately stable, contain no query or secret.
                let message = body
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("Account request failed");
                let code = match body.get("code").and_then(Value::as_str) {
                    Some("42501") => StatusCode::FORBIDDEN,
                    Some("23505") => StatusCode::CONFLICT,
                    _ => StatusCode::BAD_REQUEST,
                };
                (code, Json(json!({"error":message})))
            }
        }
        Err(_) => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"error":"Account service unavailable"})),
        ),
    }
}
macro_rules! endpoint {
    ($name:ident,$action:literal) => {
        async fn $name(
            State(c): State<Arc<Config>>,
            h: HeaderMap,
            Json(p): Json<Value>,
        ) -> (StatusCode, Json<Value>) {
            call(c, h, $action, p).await
        }
    };
}
endpoint!(reverify, "reverify");
endpoint!(guest_create, "guest_create");
endpoint!(preview, "rollover_preview");
endpoint!(rollover, "rollover_execute");
async fn self_account(State(c): State<Arc<Config>>, h: HeaderMap) -> (StatusCode, Json<Value>) {
    call(c, h, "self", json!({})).await
}
async fn guests(State(c): State<Arc<Config>>, h: HeaderMap) -> (StatusCode, Json<Value>) {
    call(c, h, "guest_list", json!({})).await
}
async fn admin_accounts(State(c): State<Arc<Config>>, h: HeaderMap) -> (StatusCode, Json<Value>) {
    call(c, h, "accounts", json!({})).await
}
async fn admin_guests(State(c): State<Arc<Config>>, h: HeaderMap) -> (StatusCode, Json<Value>) {
    call(c, h, "guest_admin_list", json!({})).await
}
async fn history(State(c): State<Arc<Config>>, h: HeaderMap) -> (StatusCode, Json<Value>) {
    call(c, h, "rollover_history", json!({})).await
}
async fn account_update(
    State(c): State<Arc<Config>>,
    Path(id): Path<String>,
    h: HeaderMap,
    Json(mut p): Json<Value>,
) -> (StatusCode, Json<Value>) {
    if !p.is_object() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error":"Object payload required"})),
        );
    }
    p["id"] = json!(id);
    call(c, h, "account_update", p).await
}
async fn guest_update(
    State(c): State<Arc<Config>>,
    Path(id): Path<String>,
    h: HeaderMap,
    Json(mut p): Json<Value>,
) -> (StatusCode, Json<Value>) {
    if !p.is_object() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error":"Object payload required"})),
        );
    }
    p["id"] = json!(id);
    call(c, h, "guest_update", p).await
}
pub fn router(config: Config) -> Router {
    Router::new()
        .route(
            "/health",
            get(|| async { Json(json!({"ok":true,"service":"accounts","candidate":true})) }),
        )
        .route("/v1/account/self", get(self_account))
        .route("/v1/account/reverify", post(reverify))
        .route("/v1/account/guest-requests", get(guests).post(guest_create))
        .route("/v1/admin/accounts", get(admin_accounts))
        .route("/v1/admin/accounts/{id}", patch(account_update))
        .route("/v1/admin/guest-requests", get(admin_guests))
        .route("/v1/admin/guest-requests/{id}", patch(guest_update))
        .route("/v1/admin/rollovers/preview", post(preview))
        .route("/v1/admin/rollovers", get(history).post(rollover))
        .layer(axum::extract::DefaultBodyLimit::max(32 * 1024))
        .with_state(Arc::new(config))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn missing_bearer_denied() {
        assert!(token(&HeaderMap::new()).is_err());
    }
    #[test]
    fn malformed_bearer_denied() {
        let mut h = HeaderMap::new();
        h.insert("authorization", "Basic secret".parse().unwrap());
        assert!(token(&h).is_err());
    }
}
