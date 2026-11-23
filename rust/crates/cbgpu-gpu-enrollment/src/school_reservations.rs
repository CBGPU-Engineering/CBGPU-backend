//! Caller-token adapter for the school-scoped SQL authority; no service-role forwarding.
use crate::{Config, bearer, fail, uuid};
use axum::{
    Json,
    body::to_bytes,
    extract::Request,
    http::{Method, StatusCode},
    response::{IntoResponse, Response},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::BTreeSet;
pub fn is_path(path: &str) -> bool {
    let p = path.trim_matches('/').split('/').collect::<Vec<_>>();
    p.len() >= 6 && p[0] == "v1" && p[1] == "schools" && p[3..6] == ["gpu", "admin", "reservations"]
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Action {
    source: String,
    action: String,
    expected_status: String,
    expected_end_time: chrono::DateTime<chrono::Utc>,
}
fn list_payload(query: Option<&str>) -> Result<Value, ()> {
    let mut seen = BTreeSet::new();
    let mut payload = json!({});
    for (k, v) in url::form_urlencoded::parse(query.unwrap_or("").as_bytes()) {
        if !seen.insert(k.to_string()) {
            return Err(());
        }
        match k.as_ref() {
            "limit" => {
                let n = v.parse::<u32>().map_err(|_| ())?;
                if !(1..=100).contains(&n) {
                    return Err(());
                }
                payload["limit"] = json!(n);
            }
            "cursor" if v.len() <= 512 => {
                payload["cursor"] = json!(v);
            }
            "q" if v.chars().count() <= 200 => {
                payload["q"] = json!(v);
            }
            "gpu" if v.len() <= 100 => {
                payload["gpu"] = json!(v);
            }
            "status"
                if [
                    "all",
                    "pending",
                    "approved",
                    "rejected",
                    "reserved",
                    "cancelled",
                    "revoked",
                    "active",
                    "upcoming",
                    "ended",
                ]
                .contains(&v.as_ref()) =>
            {
                payload["status"] = json!(v);
            }
            _ => return Err(()),
        }
    }
    Ok(payload)
}
pub async fn handle(c: &Config, request: Request) -> Response {
    let path = request.uri().path().to_owned();
    let p = path.trim_matches('/').split('/').collect::<Vec<_>>();
    if p.len() < 6
        || p[2].is_empty()
        || p[2].len() > 100
        || !p[2]
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
    {
        return fail(StatusCode::BAD_REQUEST, "INVALID_SCHOOL");
    }
    let Some(auth) = bearer(&request) else {
        return fail(StatusCode::UNAUTHORIZED, "AUTHENTICATION_REQUIRED");
    };
    let (action, mut data) = match (request.method(), p.as_slice()) {
        (&Method::GET, [_, _, _, _, _, _]) => match list_payload(request.uri().query()) {
            Ok(v) => ("list", v),
            Err(_) => return fail(StatusCode::BAD_REQUEST, "INVALID_REQUEST"),
        },
        (&Method::POST, [_, _, _, _, _, _, id, "action"])
            if uuid(id) && request.uri().query().is_none() =>
        {
            ("action", json!({"reservation_id":id}))
        }
        _ => return fail(StatusCode::NOT_FOUND, "NOT_FOUND"),
    };
    let bytes = match to_bytes(request.into_body(), 4096).await {
        Ok(v) => v,
        Err(_) => return fail(StatusCode::PAYLOAD_TOO_LARGE, "INVALID_BODY"),
    };
    if action == "list" {
        if !bytes.is_empty() {
            return fail(StatusCode::BAD_REQUEST, "INVALID_BODY");
        }
    } else {
        let Ok(dto) = serde_json::from_slice::<Action>(&bytes) else {
            return fail(StatusCode::BAD_REQUEST, "INVALID_BODY");
        };
        if !["legacy", "registry"].contains(&dto.source.as_str())
            || !["approve", "reject", "cancel", "revoke"].contains(&dto.action.as_str())
            || dto.expected_status.len() > 30
        {
            return fail(StatusCode::BAD_REQUEST, "INVALID_BODY");
        }
        data["source"] = json!(dto.source);
        data["action"] = json!(dto.action);
        data["expected_status"] = json!(dto.expected_status);
        data["expected_end_time"] = json!(dto.expected_end_time);
    }
    match c
        .client
        .get(format!("{}/auth/v1/user", c.backend_url))
        .header("apikey", &c.anon_key)
        .header("authorization", &auth)
        .send()
        .await
    {
        Ok(r) if r.status().is_success() => {}
        Ok(r) if r.status().is_server_error() || r.status().as_u16() == 429 => {
            return fail(StatusCode::SERVICE_UNAVAILABLE, "AUTH_UNAVAILABLE");
        }
        Ok(_) => return fail(StatusCode::UNAUTHORIZED, "INVALID_SESSION"),
        Err(_) => return fail(StatusCode::SERVICE_UNAVAILABLE, "AUTH_UNAVAILABLE"),
    }
    let response = c
        .client
        .post(format!(
            "{}/rest/v1/rpc/school_gpu_reservation_admin",
            c.backend_url
        ))
        .header("apikey", &c.anon_key)
        .header("authorization", auth)
        .json(&json!({"p_school_id":p[2],"p_action":action,"p_payload":data}))
        .send()
        .await;
    let mut response = match response {
        Ok(v) => v,
        Err(_) => return fail(StatusCode::SERVICE_UNAVAILABLE, "GPU_ADMIN_UNAVAILABLE"),
    };
    let status = response.status();
    let mut body = Vec::new();
    loop {
        match response.chunk().await {
            Ok(Some(v)) if body.len() + v.len() <= 256 * 1024 => body.extend_from_slice(&v),
            Ok(None) => break,
            _ => return fail(StatusCode::SERVICE_UNAVAILABLE, "GPU_ADMIN_UNAVAILABLE"),
        }
    }
    let Ok(value) = serde_json::from_slice::<Value>(&body) else {
        return fail(StatusCode::SERVICE_UNAVAILABLE, "GPU_ADMIN_UNAVAILABLE");
    };
    if status.is_success() {
        return (
            StatusCode::OK,
            [("Cache-Control", "private, no-store")],
            Json(value),
        )
            .into_response();
    }
    let code = value["code"].as_str().unwrap_or("");
    let message = value["message"].as_str().unwrap_or("");
    let http = match code {
        "42501" => StatusCode::FORBIDDEN,
        "P0002" => StatusCode::NOT_FOUND,
        "23P01" | "40001" => StatusCode::CONFLICT,
        "22023" | "22P02" | "22007" | "22008" => StatusCode::BAD_REQUEST,
        _ => StatusCode::SERVICE_UNAVAILABLE,
    };
    let safe = match message {
        "GPU_ADMIN_REQUIRED"
        | "INVALID_REQUEST"
        | "INVALID_CURSOR"
        | "INVALID_ACTION"
        | "RESERVATION_NOT_FOUND"
        | "RESERVATION_CHANGED"
        | "RESERVATION_EXPIRED"
        | "RESERVATION_NOT_PENDING"
        | "RESERVATION_NOT_RESERVED"
        | "OWNER_GPU_PERMISSION_REQUIRED"
        | "GPU_CURRENTLY_IN_USE" => message,
        _ => {
            if code == "23P01" {
                "RESERVATION_CONFLICT"
            } else {
                "GPU_ADMIN_UNAVAILABLE"
            }
        }
    };
    fail(http, safe)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exact_bounded_query() {
        assert!(list_payload(Some("limit=50&status=ended&q=old&gpu=2")).is_ok());
        for q in [
            "limit=101",
            "limit=0",
            "limit=5&limit=6",
            "unknown=x",
            "status=bogus",
        ] {
            assert!(list_payload(Some(q)).is_err());
        }
        assert!(is_path("/v1/schools/school-b/gpu/admin/reservations"));
        assert!(!is_path("/v1/schools/school-b/gpu/reservations"));
    }
}
