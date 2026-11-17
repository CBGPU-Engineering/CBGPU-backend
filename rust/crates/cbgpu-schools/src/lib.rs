//! Fresh session/tenant authorization lives in PostgreSQL; no caller identity is accepted in JSON.
use axum::{
    Json, Router,
    body::to_bytes,
    extract::{Request, State},
    http::{Method, StatusCode},
    response::{IntoResponse, Response},
};
use serde_json::{Value, json};
use std::sync::Arc;
pub const FEATURES: &[&str] = &[
    "community",
    "school_life",
    "projects",
    "contests",
    "gpu_compute",
    "ai",
    "materials",
    "manufacturing",
    "latex",
];
pub struct Config {
    pub enabled: bool,
    pub backend_url: String,
    pub anon_key: String,
    pub client: reqwest::Client,
}
pub fn router(config: Config) -> Router {
    Router::new().fallback(handle).with_state(Arc::new(config))
}
fn failure(status: StatusCode, message: &str, code: &str) -> Response {
    (status, Json(json!({"error":message,"code":code}))).into_response()
}
pub fn empty_draft() -> Value {
    json!({"school":{"official_name":"","type":"HIGH_SCHOOL","official_id":"","region":"","address":"","contact_email":"","contact_phone":""},"administrator":{"name":"","role":"STUDENT_OPERATOR","relationship":"","contact_email":"","verification_note":""},"configuration":{"display_name":"","brand_color":"#17634d","join_policy":"APPROVAL_REQUIRED","student_id_policy":"SCHOOL_DEFINED","teacher_policy":"MANUAL_REVIEW","community_policy":"MEMBERS_ONLY"},"features":["community","projects","contests"],"gpu":{"choice":"CONNECT_LATER"},"confirmed":false})
}
fn bearer(request: &Request) -> Option<String> {
    if request.headers().get_all("authorization").iter().count() != 1 {
        return None;
    }
    let value = request.headers().get("authorization")?.to_str().ok()?;
    let (scheme, token) = value.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("bearer")
        || token.is_empty()
        || token.bytes().any(|b| b.is_ascii_whitespace())
    {
        return None;
    }
    Some(value.into())
}
async fn handle(State(config): State<Arc<Config>>, request: Request) -> Response {
    if request.uri().path() == "/health" {
        return Json(json!({"ok":true,"enabled":config.enabled,"implementation":"native-rust","distributed_execution":false})).into_response();
    }
    if !config.enabled {
        return failure(
            StatusCode::NOT_FOUND,
            "School onboarding is not enabled",
            "FEATURE_DISABLED",
        );
    }
    let path = request
        .uri()
        .path()
        .trim_start_matches('/')
        .split('/')
        .collect::<Vec<_>>();
    let method = request.method().clone();
    let public = (method == Method::GET && path.as_slice() == ["v1", "features"])
        || method == Method::GET
            && path.first() == Some(&"v1")
            && path.get(1) == Some(&"schools")
            && path.len() <= 3;
    let auth = bearer(&request);
    if !public && auth.is_none() {
        return failure(
            StatusCode::UNAUTHORIZED,
            "Sign in to continue",
            "AUTHENTICATION_REQUIRED",
        );
    }
    let query = request.uri().query().unwrap_or("").to_owned();
    let query_fields: serde_json::Map<String, Value> =
        url::form_urlencoded::parse(query.as_bytes())
            .filter(|(key, _)| ["q", "limit", "offset", "status"].contains(&key.as_ref()))
            .map(|(key, value)| (key.into_owned(), Value::String(value.into_owned())))
            .collect();
    let route = match (path.as_slice(), &method) {
        (["v1", "features"], &Method::GET) => Some(("feature_catalog", {
            let fields = url::form_urlencoded::parse(query.as_bytes())
                .filter(|(key, _)| key == "school_id")
                .map(|(key, value)| (key.into_owned(), json!(value)))
                .collect();
            Value::Object(fields)
        })),
        (["v1", "schools"], &Method::GET) => {
            Some(("directory", Value::Object(query_fields.clone())))
        }
        (["v1", "schools", school], &Method::GET) => Some(("school", json!({"school_id":school}))),
        (["v1", "memberships"], &Method::GET) => Some(("memberships", json!({}))),
        (["v1", "current-school"], &Method::POST) => Some(("select_school", json!({}))),
        (["v1", "schools", school, "memberships"], &Method::POST) => {
            Some(("join", json!({"school_id":school})))
        }
        (["v1", "school-applications"], &Method::GET) => Some(("applications", json!({}))),
        (["v1", "school-applications"], &Method::POST) => {
            Some(("draft_create", json!({"defaults":empty_draft()})))
        }
        (["v1", "school-applications", id], &Method::GET) => Some(("draft_get", json!({"id":id}))),
        (["v1", "school-applications", id], &Method::PATCH) => {
            Some(("draft_save", json!({"id":id})))
        }
        (["v1", "school-applications", id, "submit"], &Method::POST) => {
            Some(("draft_submit", json!({"id":id})))
        }
        (["v1", "schools", school, "configuration"], &Method::GET) => {
            Some(("configuration", json!({"school_id":school})))
        }
        (["v1", "schools", school, "configuration"], &Method::PATCH) => {
            Some(("configure", json!({"school_id":school})))
        }
        (["v1", "schools", school, "members"], &Method::GET) => Some(("members", {
            let mut fields = query_fields.clone();
            fields.insert("school_id".into(), json!(school));
            Value::Object(fields)
        })),
        (["v1", "schools", school, "members", id], &Method::PATCH) => {
            Some(("review_member", json!({"school_id":school,"id":id})))
        }
        (["v1", "admin", "school-applications"], &Method::GET) => {
            Some(("platform_applications", Value::Object(query_fields.clone())))
        }
        (["v1", "admin", "school-applications", id, "review"], &Method::POST) => {
            Some(("platform_review", json!({"id":id})))
        }
        (["v1", "schools", school, "feature-requests"], &Method::GET) => {
            Some(("feature_requests", json!({"school_id":school})))
        }
        (["v1", "schools", school, "feature-requests"], &Method::POST) => {
            Some(("feature_request_create", json!({"school_id":school})))
        }
        (["v1", "messages"], &Method::GET) => Some(("messages", {
            let mut fields = query_fields.clone();
            for (key, value) in url::form_urlencoded::parse(query.as_bytes()) {
                if ["scope", "school_id"].contains(&key.as_ref()) {
                    fields.insert(key.into_owned(), json!(value));
                }
            }
            Value::Object(fields)
        })),
        (["v1", "messages"], &Method::POST) => Some(("message_create", json!({}))),
        (["v1", "messages", id], &Method::DELETE) => Some(("message_delete", json!({"id":id}))),
        (["v1", "messages", id, "report"], &Method::POST) => {
            Some(("message_report", json!({"id":id})))
        }
        (["v1", "message-mutes"], &Method::POST) => Some(("message_mute", json!({}))),
        (["v1", "publications"], &Method::GET) => Some(("publication_list", json!({}))),
        (["v1", "publications"], &Method::POST) => Some(("publication_save", json!({}))),
        _ => None,
    };
    let Some((action, mut payload)) = route else {
        return failure(StatusCode::NOT_FOUND, "School route not found", "NOT_FOUND");
    };
    if method != Method::GET {
        let bytes = match to_bytes(request.into_body(), 65536).await {
            Ok(v) => v,
            Err(_) => {
                return failure(
                    StatusCode::PAYLOAD_TOO_LARGE,
                    "School request is too large",
                    "INVALID_REQUEST",
                );
            }
        };
        let body: Value =
            match serde_json::from_slice(if bytes.is_empty() { b"{}" } else { &bytes }) {
                Ok(v) => v,
                Err(_) => {
                    return failure(
                        StatusCode::BAD_REQUEST,
                        "Use a JSON object",
                        "INVALID_REQUEST",
                    );
                }
            };
        let Some(object) = body.as_object() else {
            return failure(
                StatusCode::BAD_REQUEST,
                "Use a JSON object",
                "INVALID_REQUEST",
            );
        };
        // Route scope and identity are derived by this adapter/SQL, never overridable body fields.
        if object.keys().any(|k| {
            [
                "owner",
                "owner_id",
                "user_id",
                "applicant_id",
                "is_admin",
                "status",
                "verified",
                "school_roles",
            ]
            .contains(&k.as_str())
                && !(action == "review_member" && k == "school_roles")
        }) {
            return failure(
                StatusCode::BAD_REQUEST,
                "Untrusted account/permission fields",
                "INVALID_REQUEST",
            );
        }
        for (k, v) in object {
            if payload.get(k).is_some() {
                return failure(
                    StatusCode::BAD_REQUEST,
                    "Route scope cannot be overridden",
                    "INVALID_REQUEST",
                );
            }
            payload[k] = v.clone();
        }
    }
    if let Some(token) = &auth {
        match config
            .client
            .get(format!("{}/auth/v1/user", config.backend_url))
            .header("apikey", &config.anon_key)
            .header("authorization", token)
            .send()
            .await
        {
            Ok(response) if response.status().is_success() => {}
            Ok(_) => {
                return failure(
                    StatusCode::UNAUTHORIZED,
                    "Session expired",
                    "AUTHENTICATION_REQUIRED",
                );
            }
            Err(_) => {
                return failure(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "Authentication unavailable",
                    "AUTH_UNAVAILABLE",
                );
            }
        }
    }
    let rpc = if [
        "feature_requests",
        "feature_request_create",
        "messages",
        "message_create",
        "message_delete",
        "message_report",
        "message_mute",
    ]
    .contains(&action)
    {
        "cbgpu_school_extensions_api"
    } else {
        "cbgpu_school_api"
    };
    let (rpc, rpc_action) = match action {
        "publication_list" => ("cbgpu_public_project_api", "list"),
        "publication_save" => ("cbgpu_public_project_api", "save"),
        _ => (rpc, action),
    };
    let mut call = config
        .client
        .post(format!("{}/rest/v1/rpc/{rpc}", config.backend_url))
        .header("apikey", &config.anon_key)
        .json(&json!({"action":rpc_action,"payload":payload}));
    if let Some(auth) = auth {
        call = call.header("authorization", auth);
    } else {
        call = call.bearer_auth(&config.anon_key);
    }
    match call.send().await {
        Ok(response) => {
            let status = response.status();
            let value: Value = match response.json().await {
                Ok(v) => v,
                Err(_) => {
                    return failure(
                        StatusCode::BAD_GATEWAY,
                        "Invalid school response",
                        "BACKEND_INVALID",
                    );
                }
            };
            if status.is_success() {
                let status = if value.get("ok") == Some(&Value::Bool(false)) {
                    match value["code"].as_str() {
                        Some(
                            "DUPLICATE_SCHOOL" | "DUPLICATE_APPLICATION" | "REVISION_CONFLICT",
                        ) => StatusCode::CONFLICT,
                        _ => StatusCode::BAD_REQUEST,
                    }
                } else {
                    StatusCode::OK
                };
                (status, Json(value)).into_response()
            } else {
                let status = match value["code"].as_str() {
                    Some("42501") => StatusCode::FORBIDDEN,
                    Some("23505") => StatusCode::CONFLICT,
                    Some("P0002") => StatusCode::NOT_FOUND,
                    _ => StatusCode::BAD_REQUEST,
                };
                {
                    let code = value["message"]
                        .as_str()
                        .filter(|message| {
                            message.len() < 100
                                && message.bytes().all(|b| b.is_ascii_uppercase() || b == b'_')
                        })
                        .unwrap_or("SCHOOL_REQUEST_FAILED");
                    failure(status, "School request could not be completed", code)
                }
            }
        }
        Err(_) => failure(
            StatusCode::SERVICE_UNAVAILABLE,
            "School service unavailable",
            "BACKEND_UNAVAILABLE",
        ),
    }
}
