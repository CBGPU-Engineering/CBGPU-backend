//! JWT-delegated contest API: all contest writes remain caller-bound existing PostgreSQL RPC/RLS.
pub mod auth;
pub mod backend;
use axum::{
    Json, Router,
    body::to_bytes,
    extract::{Request, State},
    http::{Method, StatusCode},
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{BTreeSet, HashMap},
    sync::Arc,
    time::{Duration, Instant},
};
use subtle::ConstantTimeEq;
use tokio::sync::{Mutex, Semaphore};
pub type Result<T> = std::result::Result<T, Error>;
#[derive(Debug)]
pub struct Error {
    pub status: u16,
    pub code: &'static str,
    pub message: String,
}
pub fn fail(status: u16, code: &'static str, message: &str) -> Error {
    Error {
        status,
        code,
        message: message.into(),
    }
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}
impl std::error::Error for Error {}
impl IntoResponse for Error {
    fn into_response(self) -> Response {
        (
            StatusCode::from_u16(self.status).unwrap_or(StatusCode::SERVICE_UNAVAILABLE),
            Json(json!({"error":{"code":self.code,"message":self.message}})),
        )
            .into_response()
    }
}
#[derive(Clone, Debug)]
pub struct Principal {
    pub subject: uuid::Uuid,
    pub may_register: bool,
}
#[async_trait::async_trait]
pub trait Identity: Send + Sync {
    async fn authenticate(&self, token: &str) -> Result<Principal>;
    async fn may_register_contest(
        &self,
        _token: &str,
        _contest: uuid::Uuid,
        principal: &Principal,
    ) -> Result<bool> {
        Ok(principal.may_register)
    }
}
#[async_trait::async_trait]
pub trait ContestBackend: Send + Sync {
    async fn list(&self, token: &str, limit: u16, offset: u32) -> Result<Vec<Value>>;
    async fn detail(&self, token: &str, id: uuid::Uuid) -> Result<Value>;
    async fn register(
        &self,
        token: &str,
        id: uuid::Uuid,
        input: &RegistrationInput,
    ) -> Result<Value>;
    async fn own(&self, token: &str, id: uuid::Uuid, subject: uuid::Uuid) -> Result<Option<Value>>;
    async fn cancel(
        &self,
        token: &str,
        id: uuid::Uuid,
        subject: uuid::Uuid,
    ) -> Result<Option<Value>>;
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegistrationInput {
    pub attendee_name: String,
    #[serde(default = "empty_answers")]
    pub answers: Value,
    #[serde(default = "individual")]
    pub kind: String,
    #[serde(default)]
    pub team_name: Option<String>,
    #[serde(default)]
    pub join_code: Option<uuid::Uuid>,
}
fn empty_answers() -> Value {
    json!({})
}
fn individual() -> String {
    "individual".into()
}
impl RegistrationInput {
    pub fn validate(&self) -> Result<()> {
        if self.attendee_name.trim().is_empty()
            || self.attendee_name.trim().chars().count() > 100
            || self.attendee_name.contains('\0')
            || !self.answers.is_object()
        {
            return Err(fail(
                400,
                "invalid_registration",
                "Use a name of1–100 characters and an answers object",
            ));
        }
        if !["individual", "team_create", "team_join"].contains(&self.kind.as_str()) {
            return Err(fail(
                400,
                "invalid_kind",
                "Use individual, team_create or team_join",
            ));
        }
        if self.kind == "team_create"
            && self
                .team_name
                .as_ref()
                .is_none_or(|name| name.trim().is_empty() || name.trim().chars().count() > 80)
        {
            return Err(fail(
                400,
                "invalid_team_name",
                "Team creation requires a name of1–80 characters",
            ));
        }
        if self.kind == "team_join" && self.join_code.is_none() {
            return Err(fail(
                400,
                "invalid_join_code",
                "Team joining requires its UUID invite code",
            ));
        }
        if self.kind != "team_join" && self.join_code.is_some()
            || self.kind != "team_create" && self.team_name.is_some()
        {
            return Err(fail(
                400,
                "invalid_team_fields",
                "Use only fields belonging to the selected participation kind",
            ));
        }
        Ok(())
    }
}
struct Counter {
    since: Instant,
    read: u16,
    write: u16,
}
pub struct Api {
    pub identity: Arc<dyn Identity>,
    pub backend: Arc<dyn ContestBackend>,
    key: Vec<u8>,
    pub origins: BTreeSet<String>,
    rates: Mutex<HashMap<uuid::Uuid, Counter>>,
    slots: Semaphore,
}
impl Api {
    pub fn new(
        identity: Arc<dyn Identity>,
        backend: Arc<dyn ContestBackend>,
        key: Vec<u8>,
        origins: BTreeSet<String>,
    ) -> Result<Arc<Self>> {
        if key.len() < 32 {
            return Err(fail(
                503,
                "configuration",
                "Use a private internal key of at least32 bytes",
            ));
        }
        for origin in &origins {
            if reqwest::Url::parse(origin).ok().is_none_or(|url| {
                url.origin().ascii_serialization() != *origin
                    || !url.username().is_empty()
                    || url.password().is_some()
            }) || origin == "*"
                || origin == "null"
                || origin.ends_with('/')
                || !origin.starts_with("https://")
                || origin.contains(['\r', '\n'])
            {
                return Err(fail(
                    503,
                    "configuration",
                    "CORS origins must be explicit HTTPS origins",
                ));
            }
        }
        Ok(Arc::new(Self {
            identity,
            backend,
            key,
            origins,
            rates: Mutex::new(HashMap::new()),
            slots: Semaphore::new(16),
        }))
    }
    async fn rate(&self, subject: uuid::Uuid, write: bool) -> Result<()> {
        let mut rates = self.rates.lock().await;
        rates.retain(|_, v| v.since.elapsed() < Duration::from_secs(60));
        if rates.len() >= 4096 && !rates.contains_key(&subject) {
            return Err(fail(503, "busy", "API capacity reached"));
        }
        let counter = rates.entry(subject).or_insert(Counter {
            since: Instant::now(),
            read: 0,
            write: 0,
        });
        let (n, limit) = if write {
            (&mut counter.write, 20)
        } else {
            (&mut counter.read, 120)
        };
        if *n >= limit {
            return Err(fail(429, "rate_limited", "Retry after one minute"));
        }
        *n += 1;
        Ok(())
    }
}
pub fn router(api: Arc<Api>) -> Router {
    Router::new().fallback(handle).with_state(api)
}
fn bearer(headers: &axum::http::HeaderMap) -> Result<&str> {
    if headers.get_all("authorization").iter().count() != 1 {
        return Err(fail(
            401,
            "unauthorized",
            "One CBGPU access-token Bearer header is required",
        ));
    }
    headers
        .get("authorization")
        .and_then(|h| h.to_str().ok())
        .and_then(|value| {
            value
                .split_once(' ')
                .filter(|(scheme, _)| scheme.eq_ignore_ascii_case("Bearer"))
                .map(|(_, token)| token)
        })
        .filter(|v| {
            v.len() <= 16384 && v.split('.').count() == 3 && !v.contains(char::is_whitespace)
        })
        .ok_or_else(|| {
            fail(
                401,
                "unauthorized",
                "Use a real CBGPU access JWT, not an AI API key",
            )
        })
}
async fn handle(State(api): State<Arc<Api>>, request: Request) -> Response {
    let origin = request
        .headers()
        .get("origin")
        .and_then(|h| h.to_str().ok())
        .map(str::to_owned);
    let result = process(&api, request).await;
    let mut response = match result {
        Ok(response) => response,
        Err(error) => error.into_response(),
    };
    let headers = response.headers_mut();
    headers.insert("cache-control", "no-store".parse().unwrap());
    headers.insert("x-content-type-options", "nosniff".parse().unwrap());
    headers.insert("vary", "Origin".parse().unwrap());
    if let Some(origin) = origin.filter(|o| api.origins.contains(o)) {
        headers.insert("access-control-allow-origin", origin.parse().unwrap());
        headers.insert(
            "access-control-allow-methods",
            "GET, POST, DELETE, OPTIONS".parse().unwrap(),
        );
        headers.insert(
            "access-control-allow-headers",
            "Authorization, Content-Type".parse().unwrap(),
        );
        headers.insert("access-control-max-age", "300".parse().unwrap());
    }
    response
}
async fn process(api: &Api, request: Request) -> Result<Response> {
    if request.uri().path() == "/health" && request.method() == Method::GET {
        return Ok(Json(json!({"ok":true,"implementation":"native-rust","credential":"cbgpu-access-jwt","business_rules":"existing-caller-bound-rpc"})).into_response());
    }
    if ["x-cbgpu-owner", "x-cbgpu-resource-owner"]
        .iter()
        .any(|name| request.headers().contains_key(*name))
    {
        return Err(fail(
            400,
            "identity_override_forbidden",
            "Owner headers are not accepted; identity comes from the verified JWT",
        ));
    }
    if request
        .headers()
        .get_all("x-cbgpu-contest-key")
        .iter()
        .count()
        != 1
    {
        return Err(fail(403, "private_origin", "Private origin key required"));
    }
    let key = request
        .headers()
        .get("x-cbgpu-contest-key")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");
    if !bool::from(api.key.as_slice().ct_eq(key.as_bytes())) {
        return Err(fail(
            403,
            "private_origin",
            "Use the public CBGPU contest endpoint",
        ));
    }
    if let Some(origin) = request.headers().get("origin") {
        let origin = origin
            .to_str()
            .map_err(|_| fail(403, "origin_denied", "Invalid Origin"))?;
        if !api.origins.contains(origin) {
            return Err(fail(
                403,
                "origin_denied",
                "This browser origin is not allowed",
            ));
        }
    }
    if request.method() == Method::OPTIONS {
        return Ok(StatusCode::NO_CONTENT.into_response());
    }
    let _slot = api
        .slots
        .try_acquire()
        .map_err(|_| fail(503, "busy", "Too many concurrent requests"))?;
    let token = bearer(request.headers())?.to_owned();
    let principal = api.identity.authenticate(&token).await?;
    let method = request.method().clone();
    api.rate(principal.subject, method != Method::GET).await?;
    let path = request.uri().path();
    if path.ends_with('/') || path.contains('%') || path.contains("//") {
        return Err(fail(404, "not_found", "Endpoint not found"));
    }
    let segments: Vec<_> = path.trim_start_matches('/').split('/').collect();
    if segments.len() < 2 || segments[0] != "v1" || segments[1] != "contests" {
        return Err(fail(404, "not_found", "Endpoint not found"));
    }
    if segments.len() == 2 && method == Method::GET {
        let mut limit = 50u16;
        let mut offset = 0u32;
        for pair in request
            .uri()
            .query()
            .unwrap_or("")
            .split('&')
            .filter(|p| !p.is_empty())
        {
            let (key, value) = pair
                .split_once('=')
                .ok_or_else(|| fail(400, "invalid_query", "Use limit and offset"))?;
            match key {
                "limit" => {
                    limit = value
                        .parse()
                        .ok()
                        .filter(|n| *n > 0 && *n <= 100)
                        .ok_or_else(|| fail(400, "invalid_query", "Limit must be1–100"))?
                }
                "offset" => {
                    offset = value
                        .parse()
                        .ok()
                        .filter(|n| *n <= 10000)
                        .ok_or_else(|| fail(400, "invalid_query", "Offset exceeds10000"))?
                }
                _ => {
                    return Err(fail(
                        400,
                        "invalid_query",
                        "Only limit and offset are accepted",
                    ));
                }
            }
        }
        return Ok(Json(json!({"contests":api.backend.list(&token,limit,offset).await?,"limit":limit,"offset":offset})).into_response());
    }
    if segments.len() == 3 && segments[2] == "register" && method == Method::POST {
        let bytes = to_bytes(request.into_body(), 262144)
            .await
            .map_err(|_| fail(413, "body_too_large", "Registration body exceeds256KiB"))?;
        let mut value: Value = serde_json::from_slice(&bytes).map_err(|_| {
            fail(
                400,
                "invalid_registration",
                "Use a JSON registration object",
            )
        })?;
        let id = value
            .as_object_mut()
            .and_then(|map| map.remove("contest_id"))
            .and_then(|value| {
                value
                    .as_str()
                    .and_then(|value| uuid::Uuid::parse_str(value).ok())
            })
            .ok_or_else(|| fail(400, "invalid_contest_id", "Use a contest_id UUID"))?;
        return register_value(api, &token, &principal, id, value).await;
    }
    let id = segments
        .get(2)
        .and_then(|s| uuid::Uuid::parse_str(s).ok())
        .ok_or_else(|| fail(404, "not_found", "Contest not found"))?;
    if segments.len() == 3 && method == Method::GET {
        return Ok(Json(json!({"contest":api.backend.detail(&token,id).await?})).into_response());
    }
    if segments.len() == 4 && segments[3] == "form" && method == Method::GET {
        let contest = api.backend.detail(&token, id).await?;
        let mut form = json!({"contest_id":id,"title":contest["title"],"credential":"CBGPU access-token Bearer","registration_endpoint":format!("/api/ai/v1/contests/{id}/registrations")});
        for key in [
            "registration_questions",
            "participation_mode",
            "allow_individual_entries",
            "team_min_size",
            "team_max_size",
            "registration_opens_at",
            "registration_closes_at",
            "event_start_at",
            "registration_open",
            "approval_required",
            "waitlist_enabled",
            "capacity",
            "lifecycle_status",
        ] {
            form[key] = contest[key].clone();
        }
        return Ok(Json(form).into_response());
    }
    if segments.len() == 4 && segments[3] == "registrations" && method == Method::POST {
        let bytes = to_bytes(request.into_body(), 262144)
            .await
            .map_err(|_| fail(413, "body_too_large", "Registration body exceeds256KiB"))?;
        let value: Value = serde_json::from_slice(&bytes).map_err(|_| {
            fail(
                400,
                "invalid_registration",
                "Use a JSON registration object",
            )
        })?;
        return register_value(api, &token, &principal, id, value).await;
    }
    if segments.len() == 5 && segments[3] == "registrations" && segments[4] == "me" {
        if method == Method::GET {
            return Ok(Json(
                json!({"registration":api.backend.own(&token,id,principal.subject).await?}),
            )
            .into_response());
        }
        if method == Method::DELETE {
            let bytes = to_bytes(request.into_body(), 1)
                .await
                .map_err(|_| fail(400, "invalid_body", "Cancellation accepts no request body"))?;
            if !bytes.is_empty() {
                return Err(fail(
                    400,
                    "invalid_body",
                    "Cancellation accepts no request body",
                ));
            }
            let row = api
                .backend
                .cancel(&token, id, principal.subject)
                .await?
                .ok_or_else(|| fail(404, "not_found", "No own registration found"))?;
            return Ok(Json(json!({"registration":row})).into_response());
        }
    }
    Err(fail(404, "not_found", "Endpoint not found"))
}

async fn register_value(
    api: &Api,
    token: &str,
    principal: &Principal,
    id: uuid::Uuid,
    value: Value,
) -> Result<Response> {
    if !api
        .identity
        .may_register_contest(token, id, principal)
        .await?
    {
        return Err(fail(
            403,
            "school_permission_required",
            "This account cannot create contest registrations",
        ));
    }
    let mut input: RegistrationInput = serde_json::from_value(value).map_err(|_| {
        fail(
            400,
            "invalid_registration",
            "Use only attendee_name, answers, kind, team_name and join_code",
        )
    })?;
    input.attendee_name = input.attendee_name.trim().into();
    if let Some(name) = &mut input.team_name {
        *name = name.trim().into()
    }
    input.validate()?;
    let row = api.backend.register(token, id, &input).await?;
    if row["user_id"]
        .as_str()
        .and_then(|value| uuid::Uuid::parse_str(value).ok())
        != Some(principal.subject)
    {
        return Err(fail(
            503,
            "backend_unavailable",
            "Registration identity mismatch",
        ));
    }
    Ok((StatusCode::CREATED, Json(json!({"registration":row}))).into_response())
}
