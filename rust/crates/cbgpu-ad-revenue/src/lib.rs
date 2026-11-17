pub mod report;
use axum::{
    Json, Router,
    body::to_bytes,
    extract::{Request, State},
    http::{Method, StatusCode},
    response::{IntoResponse, Response},
};
use serde_json::{Value, json};
use std::sync::Arc;
pub struct Config {
    pub enabled: bool,
    pub origin: String,
    pub anon: String,
    pub client: reqwest::Client,
}
pub fn router(config: Config) -> Router {
    Router::new().fallback(handle).with_state(Arc::new(config))
}
fn error(status: u16, message: &str) -> Response {
    let mut r = (
        StatusCode::from_u16(status).unwrap_or(StatusCode::SERVICE_UNAVAILABLE),
        Json(json!({"error":message})),
    )
        .into_response();
    r.headers_mut()
        .insert("cache-control", "private, no-store".parse().unwrap());
    r
}
fn response(value: Value) -> Response {
    let mut r = Json(value).into_response();
    r.headers_mut()
        .insert("cache-control", "private, no-store".parse().unwrap());
    r
}
fn bearer(request: &Request) -> Option<String> {
    if request.headers().get_all("authorization").iter().count() != 1 {
        return None;
    }
    let value = request.headers().get("authorization")?.to_str().ok()?;
    let (scheme, token) = value.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("bearer")
        || token.is_empty()
        || token.bytes().any(|v| v.is_ascii_whitespace())
    {
        return None;
    }
    Some(value.into())
}
fn rpc_error(status: u16, message: &str) -> Box<Response> {
    Box::new(error(status, message))
}
async fn rpc(
    config: &Config,
    token: &str,
    action: &str,
    payload: Value,
) -> Result<Value, Box<Response>> {
    let mut reply = config
        .client
        .post(format!(
            "{}/rest/v1/rpc/cbgpu_ad_revenue_api",
            config.origin.trim_end_matches('/')
        ))
        .header("apikey", &config.anon)
        .header("authorization", token)
        .json(&json!({"action":action,"payload":payload}))
        .send()
        .await
        .map_err(|_| rpc_error(503, "Report authority/storage is unavailable"))?;
    let status = reply.status().as_u16();
    if status != 200 {
        return Err(rpc_error(
            match status {
                400 | 401 | 403 | 409 | 413 => status,
                _ => 503,
            },
            match status {
                401 => "An active authenticated session is required",
                403 => "Fresh platform administrator access is required",
                409 => "Report overlaps existing data or another import is running",
                400 | 413 => "Invalid report data",
                _ => "Report authority/storage is unavailable",
            },
        ));
    }
    if reply.content_length().is_some_and(|n| n > 2 * 1024 * 1024) {
        return Err(rpc_error(503, "Report storage response exceeds the bound"));
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = reply
        .chunk()
        .await
        .map_err(|_| rpc_error(503, "Invalid report storage response"))?
    {
        if bytes.len() + chunk.len() > 2 * 1024 * 1024 {
            return Err(rpc_error(503, "Report storage response exceeds the bound"));
        }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes).map_err(|_| rpc_error(503, "Invalid report storage response"))
}
async fn handle(State(config): State<Arc<Config>>, request: Request) -> Response {
    if request.uri().path() == "/health" {
        return response(
            json!({"ok":true,"enabled":config.enabled,"implementation":"native-rust","official_console_verified":false}),
        );
    }
    if !config.enabled {
        return error(404, "Ad revenue reporting is not enabled");
    }
    let Some(token) = bearer(&request) else {
        return error(401, "Bearer authentication required");
    };
    if let Err(e) = rpc(&config, &token, "authorize", json!({})).await {
        return *e;
    }
    let path = request.uri().path().trim_end_matches('/').to_owned();
    let method = request.method().clone();
    let base = "/v1/admin/ad-revenue";
    if path != base && !path.starts_with(&format!("{base}/")) {
        return error(404, "Unknown report route");
    }
    let suffix = path
        .strip_prefix(base)
        .unwrap_or("invalid")
        .trim_matches('/')
        .split('/')
        .filter(|v| !v.is_empty())
        .collect::<Vec<_>>();
    let (action, id) = match (suffix.as_slice(), &method) {
        ([], &Method::GET) | (["analytics"], &Method::GET) => ("analytics", None),
        (["imports"], &Method::GET) => ("imports", None),
        (["experiments"], &Method::GET) => ("experiments", None),
        (["preview"], &Method::POST) => ("preview", None),
        (["imports"], &Method::POST) => ("import", None),
        (["experiments"], &Method::POST) => ("experiment_create", None),
        (["experiments", id], &Method::PATCH) if uuid(id) => ("experiment_update", Some(*id)),
        _ => return error(404, "Unknown report route"),
    };
    let payload = if method == Method::GET {
        let mut fields = serde_json::Map::new();
        for (key, value) in request.uri().query().map(url_pairs).unwrap_or_default() {
            if !["from", "to", "kind", "timezone"].contains(&key.as_str())
                || fields.insert(key, Value::String(value)).is_some()
            {
                return error(400, "Unknown or duplicate report query");
            }
        }
        if action == "analytics"
            && (!fields
                .get("from")
                .and_then(Value::as_str)
                .is_some_and(report::valid_date)
                || !fields
                    .get("to")
                    .and_then(Value::as_str)
                    .is_some_and(report::valid_date))
        {
            return error(400, "Select an explicit YYYY-MM-DD report range");
        }
        Value::Object(fields)
    } else {
        let bytes = match tokio::time::timeout(
            std::time::Duration::from_secs(10),
            to_bytes(request.into_body(), 3 * 1024 * 1024),
        )
        .await
        {
            Ok(Ok(v)) => v,
            _ => return error(413, "Report body is too large or timed out"),
        };
        if action.starts_with("experiment_") {
            let mut body: Value = match serde_json::from_slice(&bytes) {
                Ok(Value::Object(v)) => Value::Object(v),
                _ => return error(400, "Invalid experiment metadata"),
            };
            if body.get("id").is_some() {
                return error(400, "Experiment identity comes only from its route");
            }
            if let Some(id) = id {
                body["id"] = json!(id);
            }
            body
        } else {
            let input: report::Input = match serde_json::from_slice(&bytes) {
                Ok(v) => v,
                Err(_) => return error(400, "Invalid explicit CSV mapping"),
            };
            let expected = input.preview_hash.clone();
            let confirmed = input.confirmed;
            let prepared = match report::prepare(input) {
                Ok(v) => v,
                Err(e) => return error(400, &e),
            };
            if action == "import"
                && (!confirmed || expected.as_deref() != Some(prepared.preview_hash.as_str()))
            {
                return error(409, "Preview changed or explicit confirmation is missing");
            }
            let mut payload = serde_json::to_value(prepared).expect("typed report");
            payload["confirmed"] = json!(action == "import");
            payload
        }
    };
    match rpc(&config, &token, action, payload).await {
        Ok(v) => response(v),
        Err(e) => *e,
    }
}
fn url_pairs(query: &str) -> Vec<(String, String)> {
    // reqwest uses the same URL parser; no ad-hoc percent decoding.
    let mut u = reqwest::Url::parse("http://report/").expect("fixed URL");
    u.set_query(Some(query));
    u.query_pairs()
        .map(|(a, b)| (a.into_owned(), b.into_owned()))
        .collect()
}

fn uuid(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(i, v)| {
            if [8, 13, 18, 23].contains(&i) {
                v == b'-'
            } else {
                v.is_ascii_hexdigit()
            }
        })
}
