use crate::{
    Identity, MintRequest, Persistence, RegistrationInput, Result, TokenHash, fail, generate,
    verify_jwt,
};
use axum::{
    Json, Router,
    body::to_bytes,
    extract::{Request, State},
    http::{Method, StatusCode},
    response::{IntoResponse, Response},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};
use subtle::ConstantTimeEq;
use tokio::sync::{Mutex, Semaphore};
struct Counter {
    start: Instant,
    read: u16,
    write: u16,
}
pub struct Api {
    pub identity: Arc<dyn Identity>,
    pub store: Arc<dyn Persistence>,
    key: Vec<u8>,
    rates: Mutex<HashMap<[u8; 32], Counter>>,
    slots: Semaphore,
}
impl Api {
    pub fn new(
        identity: Arc<dyn Identity>,
        store: Arc<dyn Persistence>,
        key: Vec<u8>,
    ) -> Result<Arc<Self>> {
        if key.len() < 32 {
            return Err(fail(
                503,
                "configuration",
                "Private token origin key must contain at least32 bytes",
            ));
        }
        Ok(Arc::new(Self {
            identity,
            store,
            key,
            rates: Mutex::new(HashMap::new()),
            slots: Semaphore::new(16),
        }))
    }
    async fn rate(&self, key: [u8; 32], write: bool) -> Result<()> {
        let mut rates = self.rates.lock().await;
        rates.retain(|_, v| v.start.elapsed() < Duration::from_secs(60));
        if rates.len() >= 4096 && !rates.contains_key(&key) {
            return Err(fail(503, "busy", "Token API capacity reached"));
        }
        let counter = rates.entry(key).or_insert(Counter {
            start: Instant::now(),
            read: 0,
            write: 0,
        });
        let (n, max) = if write {
            (&mut counter.write, 20)
        } else {
            (&mut counter.read, 120)
        };
        if *n >= max {
            return Err(fail(429, "rate_limited", "Retry after one minute"));
        }
        *n += 1;
        Ok(())
    }
}
pub fn router(api: Arc<Api>) -> Router {
    Router::new().fallback(handle).with_state(api)
}
fn credential(headers: &axum::http::HeaderMap) -> Result<&str> {
    if headers.get_all("authorization").iter().count() != 1 {
        return Err(fail(
            401,
            "unauthorized",
            "One Bearer credential is required",
        ));
    }
    headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split_once(' '))
        .filter(|(scheme, token)| {
            scheme.eq_ignore_ascii_case("Bearer")
                && !token.is_empty()
                && token.len() <= 16384
                && !token.contains(char::is_whitespace)
        })
        .map(|(_, token)| token)
        .ok_or_else(|| fail(401, "unauthorized", "Invalid Bearer credential"))
}
async fn json_body(request: Request) -> Result<Value> {
    let bytes = to_bytes(request.into_body(), 262144)
        .await
        .map_err(|_| fail(413, "body_too_large", "JSON body exceeds256KiB"))?;
    serde_json::from_slice(&bytes).map_err(|_| fail(400, "invalid_json", "Use a JSON object"))
}
async fn empty_body(request: Request) -> Result<()> {
    let bytes = to_bytes(request.into_body(), 1)
        .await
        .map_err(|_| fail(400, "invalid_body", "This endpoint accepts no body"))?;
    if !bytes.is_empty() {
        return Err(fail(400, "invalid_body", "This endpoint accepts no body"));
    }
    Ok(())
}
async fn handle(State(api): State<Arc<Api>>, request: Request) -> Response {
    let mut response = match process(&api, request).await {
        Ok(value) => value,
        Err(error) => error.into_response(),
    };
    for (name, value) in [
        ("cache-control", "no-store"),
        ("x-content-type-options", "nosniff"),
        ("referrer-policy", "no-referrer"),
    ] {
        response.headers_mut().insert(name, value.parse().unwrap());
    }
    response
}
async fn process(api: &Api, request: Request) -> Result<Response> {
    if request.uri().path() == "/health" && request.method() == Method::GET {
        return Ok(Json(json!({"ok":true,"candidate":true,"frozen_jwt_service_changed":false,"auth":"jwt-only token management; scoped PAT-only delegation"})).into_response());
    }
    if request
        .headers()
        .get_all("x-cbgpu-contest-key")
        .iter()
        .count()
        != 1
        || !bool::from(
            api.key.as_slice().ct_eq(
                request
                    .headers()
                    .get("x-cbgpu-contest-key")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("")
                    .as_bytes(),
            ),
        )
    {
        return Err(fail(
            403,
            "private_origin",
            "Use the approved CBGPU public proxy",
        ));
    }
    if request.headers().contains_key("origin") {
        return Err(fail(
            403,
            "private_origin",
            "Browser traffic must use the trusted public thin proxy",
        ));
    }
    if ["x-cbgpu-owner", "x-cbgpu-resource-owner"]
        .iter()
        .any(|key| request.headers().contains_key(*key))
    {
        return Err(fail(
            400,
            "identity_override_forbidden",
            "Owner headers are not accepted",
        ));
    }
    let _slot = api
        .slots
        .try_acquire()
        .map_err(|_| fail(503, "busy", "Too many token requests"))?;
    let credential = credential(request.headers())?.to_owned();
    let method = request.method().clone();
    let path = request.uri().path().to_owned();
    if path.ends_with('/') || path.contains('%') || request.uri().query().is_some() {
        return Err(fail(
            400,
            "invalid_endpoint",
            "Use exact token API paths without query overrides",
        ));
    }
    let parts: Vec<_> = path.trim_start_matches('/').split('/').collect();
    if parts.len() >= 2 && parts[..2] == ["v1", "tokens"] {
        let actor = verify_jwt(api.identity.as_ref(), &credential).await?;
        let rate = Sha256::digest(format!("token-owner:{}", actor.subject)).into();
        api.rate(rate, method != Method::GET).await?;
        if parts.len() == 2 && method == Method::GET {
            empty_body(request).await?;
            return Ok(Json(api.store.list(&actor).await?).into_response());
        }
        if parts.len() == 2 && method == Method::POST {
            let value = json_body(request).await?;
            let input: MintRequest = serde_json::from_value(value).map_err(|_| {
                fail(
                    400,
                    "invalid_token_request",
                    "Use only name, scopes, allowed_contest_ids and expires_in_days",
                )
            })?;
            input.validate()?;
            let (secret, hash) = generate();
            let record = api.store.mint(&actor, &hash, &input).await?;
            return Ok((
                StatusCode::CREATED,
                Json(json!({"token":secret,"record":record,"shown_once":true})),
            )
                .into_response());
        }
        if parts.len() == 3 && method == Method::DELETE {
            let id = parts[2]
                .parse()
                .map_err(|_| fail(404, "not_found", "Token not found"))?;
            empty_body(request).await?;
            if !api.store.revoke(&actor, id).await? {
                return Err(fail(404, "not_found", "Token not found"));
            }
            return Ok(Json(json!({"ok":true})).into_response());
        }
        return Err(fail(404, "not_found", "Token endpoint not found"));
    }
    if parts.len() < 2 || parts[..2] != ["v1", "contests"] {
        return Err(fail(404, "not_found", "Endpoint not found"));
    }
    let hash = TokenHash::parse(&credential)?;
    api.rate(hash.rate_key(), method != Method::GET).await?;
    if parts.len() == 2 && method == Method::GET {
        empty_body(request).await?;
        return Ok(Json(api.store.read(&hash, None, false).await?).into_response());
    }
    if parts.len() == 3 && parts[2] == "register" && method == Method::POST {
        let mut value = json_body(request).await?;
        let id = value
            .as_object_mut()
            .and_then(|value| value.remove("contest_id"))
            .and_then(|v| v.as_str().and_then(|v| v.parse().ok()))
            .ok_or_else(|| fail(400, "invalid_contest_id", "Use a contest_id UUID"))?;
        return register(api, &hash, id, value).await;
    }
    let id = parts
        .get(2)
        .and_then(|v| v.parse::<uuid::Uuid>().ok())
        .ok_or_else(|| fail(404, "not_found", "Contest not found"))?;
    if parts.len() == 3 && method == Method::GET {
        empty_body(request).await?;
        return Ok(Json(api.store.read(&hash, Some(id), false).await?).into_response());
    }
    if parts.len() == 4 && parts[3] == "form" && method == Method::GET {
        empty_body(request).await?;
        let value = api.store.read(&hash, Some(id), false).await?;
        let contest = &value["contest"];
        let mut form = json!({"contest_id":id,"title":contest["title"]});
        for key in [
            "registration_questions",
            "participation_mode",
            "allow_individual_entries",
            "team_min_size",
            "team_max_size",
            "registration_open",
            "registration_opens_at",
            "registration_closes_at",
            "event_start_at",
            "approval_required",
            "waitlist_enabled",
            "capacity",
            "lifecycle_status",
        ] {
            form[key] = contest[key].clone()
        }
        return Ok(Json(form).into_response());
    }
    if parts.len() == 4 && parts[3] == "registrations" && method == Method::POST {
        return register(api, &hash, id, json_body(request).await?).await;
    }
    if parts.len() == 5 && parts[3] == "registrations" && parts[4] == "me" {
        empty_body(request).await?;
        if method == Method::GET {
            return Ok(Json(api.store.read(&hash, Some(id), true).await?).into_response());
        }
        if method == Method::DELETE {
            return Ok(Json(api.store.cancel(&hash, id).await?).into_response());
        }
    }
    Err(fail(404, "not_found", "Endpoint not found"))
}
async fn register(api: &Api, hash: &TokenHash, id: uuid::Uuid, value: Value) -> Result<Response> {
    let mut input: RegistrationInput = serde_json::from_value(value).map_err(|_| {
        fail(
            400,
            "invalid_registration",
            "Only registration fields are accepted; identity and status are derived by the server",
        )
    })?;
    input.attendee_name = input.attendee_name.trim().into();
    input.validate()?;
    Ok((
        StatusCode::CREATED,
        Json(api.store.register(hash, id, &input).await?),
    )
        .into_response())
}
