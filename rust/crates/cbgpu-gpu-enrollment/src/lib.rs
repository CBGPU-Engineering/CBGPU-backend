//! Optional school GPU inventory. No scheduling, commands, execution or sharing endpoints.
pub mod commands;
pub mod notebooks;
use axum::{
    Json, Router,
    body::to_bytes,
    extract::{Request, State},
    http::{Method, StatusCode},
    response::{IntoResponse, Response},
};
use ed25519_dalek::{Signature, VerifyingKey};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::Arc;

#[derive(Clone)]
pub struct Config {
    pub enabled: bool,
    pub diagnostics: Option<Arc<ed25519_dalek::SigningKey>>,
    pub notebooks: Option<Arc<ed25519_dalek::SigningKey>>,
    pub backend_url: String,
    pub anon_key: String,
    pub service_key: String,
    pub client: reqwest::Client,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GpuHealth {
    pub index: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hardware_uuid: Option<String>,
    pub name: String,
    pub memory_total_mib: u64,
    pub memory_used_mib: u64,
    pub utilization_percent: u8,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Health {
    pub agent_version: String,
    pub os: String,
    pub gpus: Vec<GpuHealth>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Report {
    pub node_id: String,
    pub credential_version: i64,
    pub sequence: i64,
    pub reported_at: i64,
    pub health: Health,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SignedReport {
    report: Report,
    signature: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Enrollment {
    token: String,
    public_key: String,
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
pub fn unhex<const N: usize>(value: &str) -> Option<[u8; N]> {
    if value.len() != N * 2 || !value.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let mut result = [0; N];
    for (index, slot) in result.iter_mut().enumerate() {
        *slot = u8::from_str_radix(&value[index * 2..index * 2 + 2], 16).ok()?;
    }
    Some(result)
}
fn uuid(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(i, b)| {
            if [8, 13, 18, 23].contains(&i) {
                b == b'-'
            } else {
                b.is_ascii_hexdigit()
            }
        })
}
pub fn valid_hardware_uuid(value: &str) -> bool {
    value.len() == 40 && value.starts_with("GPU-") && uuid(&value[4..])
}
pub fn validate_report(r: &Report) -> bool {
    uuid(&r.node_id)
        && r.credential_version > 0
        && r.sequence > 0
        && r.reported_at > 0
        && r.health.agent_version == env!("CARGO_PKG_VERSION")
        && r.health.os == "ubuntu"
        && (1..=16).contains(&r.health.gpus.len())
        && r.health.gpus.iter().all(|g| {
            !g.name.is_empty()
                && g.name.len() <= 120
                && !g.name.chars().any(char::is_control)
                && g.hardware_uuid.as_deref().is_none_or(valid_hardware_uuid)
                && g.index < 256
                && g.memory_total_mib > 0
                && g.memory_total_mib <= 16_777_216
                && g.memory_used_mib <= g.memory_total_mib
                && g.utilization_percent <= 100
        })
        && r.health.gpus.iter().enumerate().all(|(i, g)| {
            !r.health.gpus[..i].iter().any(|other| {
                other.index == g.index
                    || (g.hardware_uuid.is_some()
                        && other
                            .hardware_uuid
                            .as_ref()
                            .zip(g.hardware_uuid.as_ref())
                            .is_some_and(|(a, b)| a.eq_ignore_ascii_case(b)))
            })
        })
}
pub fn verify_report(r: &Report, signature: &str, public_key: &str) -> bool {
    if !validate_report(r) {
        return false;
    }
    let (Some(key), Some(signature)) = (unhex::<32>(public_key), unhex::<64>(signature)) else {
        return false;
    };
    let Ok(key) = VerifyingKey::from_bytes(&key) else {
        return false;
    };
    let Ok(payload) = serde_json::to_vec(r) else {
        return false;
    };
    key.verify_strict(&payload, &Signature::from_bytes(&signature))
        .is_ok()
}
fn fail(status: StatusCode, code: &str) -> Response {
    (
        status,
        [("Cache-Control", "private, no-store")],
        Json(json!({"error":code})),
    )
        .into_response()
}
fn bearer(request: &Request) -> Option<String> {
    if request.headers().get_all("authorization").iter().count() != 1 {
        return None;
    }
    let value = request.headers().get("authorization")?.to_str().ok()?;
    let (kind, token) = value.split_once(' ')?;
    (kind.eq_ignore_ascii_case("bearer")
        && !token.is_empty()
        && token.len() <= 8192
        && !token.bytes().any(|b| b.is_ascii_whitespace()))
    .then(|| value.to_owned())
}
async fn rpc(
    c: &Config,
    name: &str,
    payload: Value,
    auth: Option<&str>,
    privileged: bool,
) -> Result<Value, StatusCode> {
    let key = if privileged {
        &c.service_key
    } else {
        &c.anon_key
    };
    let mut response = c
        .client
        .post(format!("{}/rest/v1/rpc/{name}", c.backend_url))
        .header("apikey", key)
        .header(
            "authorization",
            auth.map(String::from)
                .unwrap_or_else(|| format!("Bearer {key}")),
        )
        .json(&payload)
        .send()
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    if !response.status().is_success() {
        return Err(if response.status().is_server_error() {
            StatusCode::SERVICE_UNAVAILABLE
        } else {
            StatusCode::FORBIDDEN
        });
    }
    if response.content_length().is_some_and(|n| n > 256 * 1024) {
        return Err(StatusCode::SERVICE_UNAVAILABLE);
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?
    {
        if bytes.len() + chunk.len() > 256 * 1024 {
            return Err(StatusCode::SERVICE_UNAVAILABLE);
        }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes).map_err(|_| StatusCode::SERVICE_UNAVAILABLE)
}
pub fn router(config: Config) -> Router {
    Router::new().fallback(handle).with_state(Arc::new(config))
}
async fn handle(State(c): State<Arc<Config>>, request: Request) -> Response {
    if request.uri().path() == "/health" {
        return Json(json!({"ok":true,"enabled":c.enabled,"implementation":"native-rust","execution":false,"sharing":false,"diagnostics_enabled":c.diagnostics.is_some(),"execution_ready":false,"notebooks_enabled":c.notebooks.is_some(),"notebook_signing_public_key":c.notebooks.as_ref().map(|k|hex(k.verifying_key().as_bytes()))})).into_response();
    }
    if !c.enabled {
        return fail(StatusCode::NOT_FOUND, "FEATURE_DISABLED");
    }
    let path = request.uri().path().to_owned();
    let parts = path.trim_matches('/').split('/').collect::<Vec<_>>();
    let method = request.method().clone();
    let auth = bearer(&request);
    if request.uri().query().is_some() {
        return fail(StatusCode::BAD_REQUEST, "QUERY_NOT_SUPPORTED");
    }
    if path.starts_with("/v1/gpu/notebooks/") {
        return notebooks::device(&c, request).await;
    }
    if path.starts_with("/v1/gpu/commands/") {
        return commands::device(&c, request).await;
    }
    let bytes = match to_bytes(request.into_body(), 16 * 1024).await {
        Ok(b) => b,
        Err(_) => return fail(StatusCode::PAYLOAD_TOO_LARGE, "BODY_TOO_LARGE"),
    };
    let payload = if bytes.is_empty() {
        json!({})
    } else {
        match serde_json::from_slice::<Value>(&bytes) {
            Ok(v) if v.is_object() => v,
            _ => return fail(StatusCode::BAD_REQUEST, "INVALID_BODY"),
        }
    };
    let result = if method == Method::POST && path == "/v1/gpu/enroll" {
        let Ok(e) = serde_json::from_value::<Enrollment>(payload) else {
            return fail(StatusCode::BAD_REQUEST, "INVALID_ENROLLMENT");
        };
        if !e.token.starts_with("gpuenr_")
            || e.token.len() != 71
            || unhex::<32>(&e.token[7..]).is_none()
            || unhex::<32>(&e.public_key).is_none()
        {
            return fail(StatusCode::BAD_REQUEST, "INVALID_ENROLLMENT");
        }
        rpc(
            &c,
            "school_gpu_enroll",
            json!({"p_token":e.token,"p_public_key":e.public_key}),
            None,
            false,
        )
        .await
    } else if method == Method::POST && path == "/v1/gpu/report" {
        let Ok(s) = serde_json::from_value::<SignedReport>(payload) else {
            return fail(StatusCode::BAD_REQUEST, "INVALID_REPORT");
        };
        if !validate_report(&s.report) {
            return fail(StatusCode::BAD_REQUEST, "INVALID_REPORT");
        }
        let identity = match rpc(
            &c,
            "school_gpu_node_identity",
            json!({"p_node_id":s.report.node_id}),
            None,
            true,
        )
        .await
        {
            Ok(v) => v,
            Err(status) => return fail(status, "NODE_UNAVAILABLE"),
        };
        if identity["credential_version"].as_i64() != Some(s.report.credential_version)
            || !verify_report(
                &s.report,
                &s.signature,
                identity["public_key"].as_str().unwrap_or(""),
            )
        {
            return fail(StatusCode::FORBIDDEN, "INVALID_NODE_PROOF");
        }
        rpc(&c,"school_gpu_store_resource_report",json!({"p_node_id":s.report.node_id,"p_version":s.report.credential_version,"p_sequence":s.report.sequence,"p_reported_at":s.report.reported_at,"p_health":s.report.health}),None,true).await
    } else if parts.len() >= 5 && parts[0] == "v1" && parts[1] == "schools" && parts[3] == "gpu" {
        let Some(auth) = auth else {
            return fail(StatusCode::UNAUTHORIZED, "AUTHENTICATION_REQUIRED");
        };
        if parts[2].is_empty()
            || parts[2].len() > 100
            || !parts[2]
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
        {
            return fail(StatusCode::BAD_REQUEST, "INVALID_SCHOOL");
        }
        let (action, data, resource) = match (&method, parts.as_slice()) {
            (&Method::POST, [_, _, _, _, "enrollments"]) => ("mint", payload, false),
            (&Method::GET, [_, _, _, _, "nodes"]) => ("list", json!({}), false),
            (&Method::POST, [_, _, _, _, "nodes", id, "revoke"])
                if uuid(id) && payload.as_object().is_some_and(|o| o.is_empty()) =>
            {
                ("revoke", json!({"node_id":id}), false)
            }
            (&Method::POST, [_, _, _, _, "nodes", id, "diagnostics"])
                if uuid(id) && c.diagnostics.is_some() =>
            {
                let Some(fields) = payload.as_object() else {
                    return fail(StatusCode::BAD_REQUEST, "INVALID_BODY");
                };
                if fields.len() != 1 || !fields.contains_key("idempotency_key") {
                    return fail(StatusCode::BAD_REQUEST, "INVALID_BODY");
                }
                let mut data = payload.clone();
                data["node_id"] = json!(id);
                ("diagnostic_create", data, false)
            }
            (&Method::GET, [_, _, _, _, "nodes", id, "diagnostics"])
                if uuid(id) && c.diagnostics.is_some() =>
            {
                ("diagnostic_list", json!({"node_id":id}), false)
            }
            (&Method::GET, [_, _, _, _, "reservations", id, "notebook"])
                if uuid(id) && c.notebooks.is_some() =>
            {
                ("notebook_status", json!({"reservation_id":id}), false)
            }
            (&Method::POST, [_, _, _, _, "reservations", id, op])
                if uuid(id)
                    && ["launch", "stop", "session"].contains(op)
                    && c.notebooks.is_some() =>
            {
                let allowed = if *op == "launch" {
                    vec!["client_request_id", "temporary_storage_consent"]
                } else if *op == "stop" {
                    vec!["client_request_id"]
                } else {
                    vec![]
                };
                if !payload
                    .as_object()
                    .is_some_and(|o| o.keys().all(|k| allowed.contains(&k.as_str())))
                {
                    return fail(StatusCode::BAD_REQUEST, "INVALID_BODY");
                }
                let mut data = payload.clone();
                data["reservation_id"] = json!(id);
                (
                    match *op {
                        "launch" => "notebook_launch",
                        "stop" => "notebook_stop",
                        _ => "notebook_session",
                    },
                    data,
                    false,
                )
            }
            (&Method::GET, [_, _, _, _, "resources"]) => ("list", json!({}), true),
            (&Method::GET, [_, _, _, _, "reservations"]) => ("own", json!({}), true),
            (&Method::POST, [_, _, _, _, "resources", id, "reservations"]) if uuid(id) => {
                let Some(fields) = payload.as_object() else {
                    return fail(StatusCode::BAD_REQUEST, "INVALID_BODY");
                };
                if fields
                    .keys()
                    .any(|k| !["starts_at", "ends_at"].contains(&k.as_str()))
                {
                    return fail(StatusCode::BAD_REQUEST, "INVALID_BODY");
                }
                let mut data = payload.clone();
                data["resource_id"] = json!(id);
                ("reserve", data, true)
            }
            (&Method::DELETE, [_, _, _, _, "reservations", id])
                if uuid(id) && payload.as_object().is_some_and(|o| o.is_empty()) =>
            {
                ("cancel", json!({"reservation_id":id}), true)
            }
            _ => return fail(StatusCode::NOT_FOUND, "NOT_FOUND"),
        };
        // Fresh Auth check plus DB auth.sessions and school-role authorization; never user_metadata.
        match c
            .client
            .get(format!("{}/auth/v1/user", c.backend_url))
            .header("apikey", &c.anon_key)
            .header("authorization", &auth)
            .send()
            .await
        {
            Ok(r) if r.status().is_success() => {}
            Ok(_) => return fail(StatusCode::UNAUTHORIZED, "INVALID_SESSION"),
            Err(_) => return fail(StatusCode::SERVICE_UNAVAILABLE, "AUTH_UNAVAILABLE"),
        }
        rpc(
            &c,
            if action.starts_with("notebook_"){ "school_gpu_notebook_api" } else if action.starts_with("diagnostic_"){ "school_gpu_diagnostic_admin" } else if resource {
                "school_gpu_resource_api"
            } else {
                "school_gpu_admin"
            },
            json!({"p_school_id":parts[2],"p_action":action.strip_prefix("notebook_").or_else(||action.strip_prefix("diagnostic_")).unwrap_or(action),"p_payload":data}),
            Some(&auth),
            false,
        )
        .await
    } else {
        return fail(StatusCode::NOT_FOUND, "NOT_FOUND");
    };
    let result = if path.ends_with("/enrollments")
        && method == Method::POST
        && let Some(notebook_key) = c.notebooks.as_ref()
    {
        match result {
            Ok(mut v) => match rpc(&c, "school_gpu_notebook_trust", json!({}), None, true).await {
                Ok(policy) => {
                    if policy["enabled"].as_bool() == Some(true) {
                        v["notebook_trust"] = json!({"protocol":3,"server_public_key":hex(notebook_key.verifying_key().as_bytes()),"approved_image_digest":policy["image_digest"]});
                    }
                    Ok(v)
                }
                Err(_) => Ok(v),
            },
            Err(e) => Err(e),
        }
    } else {
        result
    };
    let result = result.map(|mut v| {
        if path.ends_with("/diagnostics")
            && let Some(key) = c.diagnostics.as_ref()
        {
            v["central_signing_public_key"] = json!(hex(key.verifying_key().as_bytes()));
            v["capability"] = json!("DIAGNOSTICS_ONLY");
            v["execution_ready"] = json!(false);
        }
        v
    });
    match result {
        Ok(v) => (
            [
                ("Cache-Control", "no-store"),
                ("X-Content-Type-Options", "nosniff"),
            ],
            Json(v),
        )
            .into_response(),
        Err(status) => fail(status, "GPU_REGISTRY_REQUEST_FAILED"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};
    fn report() -> Report {
        Report {
            node_id: "12345678-1234-1234-1234-123456789abc".into(),
            credential_version: 1,
            sequence: 1,
            reported_at: 1000,
            health: Health {
                agent_version: env!("CARGO_PKG_VERSION").into(),
                os: "ubuntu".into(),
                gpus: vec![GpuHealth {
                    index: 0,
                    hardware_uuid: None,
                    name: "GPU".into(),
                    memory_total_mib: 100,
                    memory_used_mib: 50,
                    utilization_percent: 30,
                }],
            },
        }
    }
    #[test]
    fn signature_binds_every_field() {
        let key = SigningKey::from_bytes(&[7; 32]);
        let mut r = report();
        let signature = hex(&key.sign(&serde_json::to_vec(&r).unwrap()).to_bytes());
        assert!(verify_report(
            &r,
            &signature,
            &hex(key.verifying_key().as_bytes())
        ));
        r.sequence += 1;
        assert!(!verify_report(
            &r,
            &signature,
            &hex(key.verifying_key().as_bytes())
        ));
        assert!(!verify_report(
            &report(),
            &signature,
            &hex(SigningKey::from_bytes(&[8; 32]).verifying_key().as_bytes())
        ));
    }
    #[test]
    fn bounded_inventory_not_command_channel() {
        let mut r = report();
        assert!(validate_report(&r));
        r.health.gpus[0].utilization_percent = 101;
        assert!(!validate_report(&r));
        let mut v = serde_json::to_value(report()).unwrap();
        v["command"] = json!("sh");
        assert!(serde_json::from_value::<Report>(v).is_err());
        assert!(unhex::<32>(&"a".repeat(65)).is_none());
    }
    #[tokio::test]
    async fn disabled_is_fail_closed() {
        use tower::ServiceExt;
        let app = router(Config {
            enabled: false,
            diagnostics: None,
            notebooks: None,
            backend_url: "http://127.0.0.1:1".into(),
            anon_key: "not-a-secret".into(),
            service_key: "not-a-secret".into(),
            client: reqwest::Client::new(),
        });
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/v1/gpu/enroll")
                    .method("POST")
                    .body(axum::body::Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
}
