//! Anonymous-only DTO boundary. Never forwards browser cookies or user credentials.
use axum::{
    Json, Router,
    extract::{Request, State},
    http::{Method, StatusCode, header},
    response::{IntoResponse, Response},
};
use serde_json::{Value, json};
use std::{
    sync::{Arc, Mutex},
    time::Instant,
};
pub struct Config {
    pub backend_url: String,
    pub anon_key: String,
    pub client: reqwest::Client,
}
struct PublicState {
    config: Config,
    budget: Mutex<(Instant, u32)>,
}
pub fn router(config: Config) -> Router {
    Router::new()
        .fallback(handle)
        .with_state(Arc::new(PublicState {
            config,
            budget: Mutex::new((Instant::now(), 0)),
        }))
}
fn error(status: StatusCode, code: &str) -> Response {
    let mut r = (status, Json(json!({"error":code}))).into_response();
    r.headers_mut()
        .insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
    r
}
const PROJECT_FIELDS: &[&str] = &[
    "id",
    "slug",
    "title",
    "summary",
    "body_markdown",
    "description",
    "school_id",
    "school_name",
    "updated_at",
    "created_at",
    "visibility",
];
const CONTEST_FIELDS: &[&str] = &[
    "id",
    "title",
    "description",
    "organizer",
    "school_id",
    "school_name",
    "event_start_at",
    "event_end_at",
    "registration_opens_at",
    "registration_closes_at",
    "starts_at",
    "ends_at",
    "created_at",
    "location_type",
    "location_name",
    "participation_mode",
    "team_min_size",
    "team_max_size",
    "allow_individual_entries",
    "registration_open",
    "lifecycle_status",
    "eligibility",
    "visibility",
];
const SCHOOL_FIELDS: &[&str] = &[
    "id",
    "official_name",
    "display_name",
    "type",
    "region",
    "status",
    "brand_color",
    "public_description",
    "ads_enabled",
];
const COMMUNITY_FIELDS: &[&str] = &[
    "id",
    "title",
    "body",
    "created_at",
    "updated_at",
    "school_name",
    "board_id",
    "board_name",
    "display_name",
    "visibility",
];
fn project(value: &Value, fields: &[&str]) -> Option<Value> {
    let object = value.as_object()?;
    let mut out = serde_json::Map::new();
    for field in fields {
        if let Some(value) = object.get(*field) {
            if value.is_array() || value.is_object() {
                return None;
            }
            out.insert((*field).into(), value.clone());
        }
    }
    Some(Value::Object(out))
}
fn clean_response(value: Value, fields: &[&str], detail: bool) -> Option<Value> {
    if detail {
        let item = value.get("item")?;
        if item.is_null() {
            return Some(json!({"item":null}));
        }
        return Some(json!({"item":project(item,fields)?}));
    }
    let items = value.get("items")?.as_array()?;
    if items.len() > 100 {
        return None;
    }
    let rows = items
        .iter()
        .map(|v| project(v, fields))
        .collect::<Option<Vec<_>>>()?;
    Some(json!({"items":rows,"total":value["total"].as_u64().unwrap_or(items.len() as u64)}))
}
async fn handle(State(state): State<Arc<PublicState>>, request: Request) -> Response {
    if request.uri().path() == "/health" {
        return Json(
            json!({"ok":true,"implementation":"native-rust","mode":"anonymous-projection-only"}),
        )
        .into_response();
    }
    if request.method() != Method::GET {
        return error(StatusCode::METHOD_NOT_ALLOWED, "READ_ONLY");
    }
    {
        let Ok(mut budget) = state.budget.lock() else {
            return error(
                StatusCode::SERVICE_UNAVAILABLE,
                "PUBLIC_CONTENT_UNAVAILABLE",
            );
        };
        if budget.0.elapsed().as_secs() >= 60 {
            *budget = (Instant::now(), 0)
        }
        if budget.1 >= 600 {
            return error(StatusCode::TOO_MANY_REQUESTS, "PUBLIC_RATE_LIMIT");
        };
        budget.1 += 1;
    }
    let parts = request
        .uri()
        .path()
        .trim_start_matches('/')
        .split('/')
        .collect::<Vec<_>>();
    if parts.len() < 2 || parts.len() > 3 || parts[0] != "v1" {
        return error(StatusCode::NOT_FOUND, "NOT_FOUND");
    }
    let detail = parts.len() == 3;
    let (action, fields) = match (parts[1], detail) {
        ("projects", false) => ("projects_list", PROJECT_FIELDS),
        ("projects", true) => ("project", PROJECT_FIELDS),
        ("contests", false) => ("contests_list", CONTEST_FIELDS),
        ("contests", true) => ("contest", CONTEST_FIELDS),
        ("schools", false) => ("schools_list", SCHOOL_FIELDS),
        ("schools", true) => ("school", SCHOOL_FIELDS),
        ("community", false) => ("messages_list", COMMUNITY_FIELDS),
        _ => return error(StatusCode::NOT_FOUND, "NOT_FOUND"),
    };
    let mut payload = json!({"limit":24,"offset":0});
    if detail {
        let id = parts[2];
        if parts[1] == "schools" {
            if id.is_empty()
                || id.len() > 100
                || !id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
            {
                return error(StatusCode::NOT_FOUND, "NOT_FOUND");
            }
        } else if uuid::Uuid::parse_str(id).is_err() {
            return error(StatusCode::NOT_FOUND, "NOT_FOUND");
        };
        payload["id"] = json!(id);
    }
    let mut seen = std::collections::BTreeSet::new();
    for (key, value) in url::form_urlencoded::parse(request.uri().query().unwrap_or("").as_bytes())
    {
        if !seen.insert(key.to_string()) {
            return error(StatusCode::BAD_REQUEST, "INVALID_QUERY");
        }
        match key.as_ref() {
            "limit" | "offset" => {
                let Ok(n) = value.parse::<u32>() else {
                    return error(StatusCode::BAD_REQUEST, "INVALID_QUERY");
                };
                if (key == "limit" && !(1..=100).contains(&n)) || (key == "offset" && n > 10000) {
                    return error(StatusCode::BAD_REQUEST, "INVALID_QUERY");
                };
                payload[key.to_string()] = json!(n);
            }
            "query" => {
                if value.len() > 200 {
                    return error(StatusCode::BAD_REQUEST, "INVALID_QUERY");
                };
                payload["query"] = json!(value)
            }
            _ => return error(StatusCode::BAD_REQUEST, "INVALID_QUERY"),
        }
    }
    // Credentials from the browser are deliberately ignored. The only upstream identity is anon.
    let config = &state.config;
    let response = match config
        .client
        .post(format!(
            "{}/rest/v1/rpc/cbgpu_public_content",
            config.backend_url
        ))
        .header("apikey", &config.anon_key)
        .bearer_auth(&config.anon_key)
        .json(&json!({"action":action,"payload":payload}))
        .send()
        .await
    {
        Ok(r) if r.status().is_success() => r,
        _ => {
            return error(
                StatusCode::SERVICE_UNAVAILABLE,
                "PUBLIC_CONTENT_UNAVAILABLE",
            );
        }
    };
    let mut response = response;
    let mut bytes = Vec::new();
    loop {
        match response.chunk().await {
            Ok(Some(chunk)) => {
                if bytes.len().saturating_add(chunk.len()) > 1024 * 1024 {
                    return error(StatusCode::BAD_GATEWAY, "INVALID_PUBLIC_RESPONSE");
                };
                bytes.extend_from_slice(&chunk)
            }
            Ok(None) => break,
            Err(_) => return error(StatusCode::BAD_GATEWAY, "INVALID_PUBLIC_RESPONSE"),
        }
    }
    let value = match serde_json::from_slice(&bytes)
        .ok()
        .and_then(|v| clean_response(v, fields, detail))
    {
        Some(v) => v,
        None => return error(StatusCode::BAD_GATEWAY, "INVALID_PUBLIC_RESPONSE"),
    };
    if detail && value["item"].is_null() {
        return error(StatusCode::NOT_FOUND, "NOT_FOUND");
    }
    let mut response = Json(value).into_response();
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
    response
        .headers_mut()
        .insert(header::X_CONTENT_TYPE_OPTIONS, "nosniff".parse().unwrap());
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn projection_drops_identity_and_infrastructure() {
        let v = json!({"items":[{"id":"example","title":"Public","email":"private","user_id":"hidden","env_vars":{"SECRET":"hidden"},"host_port":123,"git_url":"private","description":"Opt-in description"}]});
        let result = clean_response(v, PROJECT_FIELDS, false).unwrap();
        assert_eq!(
            result,
            json!({"items":[{"id":"example","title":"Public","description":"Opt-in description"}],"total":1})
        );
    }
    #[test]
    fn malformed_or_oversized_results_fail_closed() {
        assert!(
            clean_response(json!({"rows":[{"email":"private"}]}), PROJECT_FIELDS, false).is_none()
        );
        assert!(
            clean_response(
                json!({"items":vec![json!({"id":"x"});101]}),
                PROJECT_FIELDS,
                false
            )
            .is_none()
        );
        assert!(clean_response(json!({"item":["bad"]}), PROJECT_FIELDS, true).is_none());
    }
    #[tokio::test]
    async fn unknown_mutation_and_query_never_call_backend() {
        use tower::ServiceExt;
        let client = reqwest::Client::new();
        let app = router(Config {
            backend_url: "http://127.0.0.1:1".into(),
            anon_key: "synthetic".into(),
            client,
        });
        for (method, path, status) in [
            ("POST", "/v1/projects", 405),
            ("GET", "/v1/projects?user_id=victim", 400),
            ("GET", "/v1/projects?limit=2&limit=4", 400),
            ("GET", "/v1/projects/not-a-uuid", 404),
            ("GET", "/v1/gpu", 404),
        ] {
            let response = app
                .clone()
                .oneshot(
                    axum::http::Request::builder()
                        .method(method)
                        .uri(path)
                        .body(axum::body::Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status().as_u16(), status);
            assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        }
    }
    #[tokio::test]
    async fn upstream_is_always_anonymous_and_cache_cannot_mix_account_data() {
        use tower::ServiceExt;
        let upstream=Router::new().route("/rest/v1/rpc/cbgpu_public_content",axum::routing::post(|headers:axum::http::HeaderMap,Json(body):Json<Value>|async move{
            assert_eq!(headers[header::AUTHORIZATION],"Bearer synthetic-anon");
            assert_eq!(headers["apikey"],"synthetic-anon");assert!(headers.get(header::COOKIE).is_none());
            assert_eq!(body["action"],"projects_list");
            Json(json!({"items":[{"id":"public","title":"Owner published","email":"must-drop","env_vars":{"secret":"must-drop"}}],"total":1}))
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let task = tokio::spawn(async move { axum::serve(listener, upstream).await.unwrap() });
        let app = router(Config {
            backend_url: format!("http://{address}"),
            anon_key: "synthetic-anon".into(),
            client: reqwest::Client::builder().no_proxy().build().unwrap(),
        });
        let response = app
            .oneshot(
                axum::http::Request::builder()
                    .uri("/v1/projects")
                    .header(header::AUTHORIZATION, "Bearer user-secret")
                    .header(header::COOKIE, "session=user-secret")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        assert!(response.headers().get(header::SET_COOKIE).is_none());
        let bytes = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap();
        let value: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(
            value,
            json!({"items":[{"id":"public","title":"Owner published"}],"total":1})
        );
        task.abort();
    }
}
