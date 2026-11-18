pub mod protocol;
use crate::{Config, fail, rpc, uuid};
use axum::{
    Json,
    body::to_bytes,
    extract::Request,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use protocol::*;
use serde_json::json;
use std::time::{SystemTime, UNIX_EPOCH};
pub async fn device(c: &Config, request: Request) -> Response {
    let Some(key) = c.notebooks.as_ref() else {
        return fail(StatusCode::NOT_FOUND, "NOTEBOOKS_DISABLED");
    };
    let path = request.uri().path().to_owned();
    let method = request.method().as_str().to_owned();
    if method != "POST" || request.uri().query().is_some() {
        return fail(StatusCode::BAD_REQUEST, "INVALID_NOTEBOOK_ROUTE");
    }
    let bytes = match to_bytes(request.into_body(), 24000).await {
        Ok(v) => v,
        Err(_) => return fail(StatusCode::PAYLOAD_TOO_LARGE, "INVALID_BODY"),
    };
    let s: SignedNodeRequest = match serde_json::from_slice(&bytes) {
        Ok(v) => v,
        Err(_) => return fail(StatusCode::BAD_REQUEST, "INVALID_BODY"),
    };
    let r = &s.request;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_secs() as i64;
    let action = match &r.payload {
        Payload::Capabilities {
            executor_protocol: 3,
            image_digest,
            docker_ready: _,
            toolkit_ready: _,
            hardware_uuids,
        } if image_digest.len() <= 255
            && image_digest.contains("@sha256:")
            && (1..=16).contains(&hardware_uuids.len()) =>
        {
            "capabilities"
        }
        Payload::Claim {} => "claim",
        Payload::Renew {
            command_id,
            lease_epoch,
            lease_token,
        } if uuid(command_id) && *lease_epoch > 0 && crate::unhex::<32>(lease_token).is_some() => {
            "renew"
        }
        Payload::Result {
            command_id,
            lease_epoch,
            lease_token,
            result,
        } if uuid(command_id)
            && *lease_epoch > 0
            && crate::unhex::<32>(lease_token).is_some()
            && uuid(&result.job_id)
            && result.generation == result.job_id
            && ["RUNNING", "STOPPED", "FAILED"].contains(&result.state.as_str())
            && result.error_code.as_ref().is_none_or(|e| {
                !e.is_empty()
                    && e.len() <= 64
                    && e.bytes()
                        .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
            }) =>
        {
            "result"
        }
        _ => return fail(StatusCode::BAD_REQUEST, "INVALID_PAYLOAD"),
    };
    if r.protocol != 3
        || !uuid(&r.node_id)
        || r.credential_version < 1
        || r.sequence < 1
        || r.issued_at.abs_diff(now) > 90
        || r.method != method
        || r.path != path
        || path != format!("/v1/gpu/notebooks/{action}")
    {
        return fail(StatusCode::FORBIDDEN, "INVALID_BINDING");
    }
    let identity = match rpc(
        c,
        "school_gpu_node_identity",
        json!({"p_node_id":r.node_id}),
        None,
        true,
    )
    .await
    {
        Ok(v) => v,
        Err(e) => return fail(e, "NODE_UNAVAILABLE"),
    };
    if identity["credential_version"].as_i64() != Some(r.credential_version)
        || !verify_node_request(
            r,
            &s.signature,
            identity["public_key"].as_str().unwrap_or(""),
        )
    {
        return fail(StatusCode::FORBIDDEN, "INVALID_NODE_PROOF");
    }
    let value=match rpc(c,"school_gpu_notebook_device",json!({"p_node_id":r.node_id,"p_version":r.credential_version,"p_sequence":r.sequence,"p_issued_at":r.issued_at,"p_action":action,"p_payload":r.payload}),None,true).await{Ok(v)=>v,Err(e)=>return fail(e,"NOTEBOOK_REQUEST_DENIED")};
    let command = if value["command"].is_null() {
        None
    } else {
        match serde_json::from_value(value["command"].clone()) {
            Ok(v) => Some(v),
            Err(_) => return fail(StatusCode::BAD_GATEWAY, "INVALID_LEASE_RESPONSE"),
        }
    };
    (
        [("Cache-Control", "private, no-store")],
        Json(sign_envelope(
            Envelope {
                protocol: 3,
                credential_version: r.credential_version,
                node_id: r.node_id.clone(),
                request_sequence: r.sequence,
                request_method: method,
                request_path: path,
                issued_at: now,
                expires_at: now + 90,
                command,
            },
            key,
        )),
    )
        .into_response()
}

/// Only the authenticated job owner can exchange a job URL for a fresh notebook grant.
/// School/reservation/owner are resolved by the SQL authority, never browser parameters.
pub async fn owner_session(c: &Config, request: Request, job: &str) -> Response {
    if c.notebooks.is_none() {
        return fail(StatusCode::NOT_FOUND, "NOTEBOOKS_DISABLED");
    }
    if request.method() != "POST" || request.uri().query().is_some() {
        return fail(StatusCode::BAD_REQUEST, "INVALID_ROUTE");
    }
    let Some(auth) = crate::bearer(&request) else {
        return fail(StatusCode::UNAUTHORIZED, "AUTHENTICATION_REQUIRED");
    };
    let bytes = match to_bytes(request.into_body(), 1024).await {
        Ok(b) => b,
        Err(_) => return fail(StatusCode::PAYLOAD_TOO_LARGE, "INVALID_BODY"),
    };
    if !bytes.is_empty()
        && !serde_json::from_slice::<serde_json::Value>(&bytes)
            .is_ok_and(|v| v.as_object().is_some_and(|o| o.is_empty()))
    {
        return fail(StatusCode::BAD_REQUEST, "INVALID_BODY");
    }
    match rpc(
        c,
        "school_gpu_notebook_owner_session",
        json!({"p_job_id":job}),
        Some(&auth),
        false,
    )
    .await
    {
        Ok(value) => ([("Cache-Control", "private, no-store")], Json(value)).into_response(),
        Err(status) => fail(status, "NOTEBOOK_SESSION_DENIED"),
    }
}
