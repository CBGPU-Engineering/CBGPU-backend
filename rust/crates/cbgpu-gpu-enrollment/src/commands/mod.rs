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
    let Some(key) = c.diagnostics.as_ref() else {
        return fail(StatusCode::NOT_FOUND, "DIAGNOSTICS_DISABLED");
    };
    let path = request.uri().path().to_owned();
    let method = request.method().as_str().to_owned();
    if method != "POST" || request.uri().query().is_some() {
        return fail(StatusCode::BAD_REQUEST, "INVALID_COMMAND_ROUTE");
    }
    let bytes = match to_bytes(request.into_body(), 24000).await {
        Ok(v) => v,
        Err(_) => return fail(StatusCode::PAYLOAD_TOO_LARGE, "INVALID_COMMAND_BODY"),
    };
    let s: SignedNodeRequest = match serde_json::from_slice(&bytes) {
        Ok(v) => v,
        Err(_) => return fail(StatusCode::BAD_REQUEST, "INVALID_COMMAND_BODY"),
    };
    let r = &s.request;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_secs() as i64;
    let action = match &r.payload {
        CommandPayload::Claim { agent_protocol: 2 } => "claim",
        CommandPayload::Renew {
            command_id,
            lease_epoch,
            lease_token,
        } if uuid(command_id) && *lease_epoch > 0 && crate::unhex::<32>(lease_token).is_some() => {
            "renew"
        }
        CommandPayload::Result {
            command_id,
            lease_epoch,
            lease_token,
            result,
        } if uuid(command_id)
            && *lease_epoch > 0
            && crate::unhex::<32>(lease_token).is_some()
            && valid_result(result) =>
        {
            "result"
        }
        _ => return fail(StatusCode::BAD_REQUEST, "INVALID_COMMAND_PAYLOAD"),
    };
    if r.protocol != 2
        || !uuid(&r.node_id)
        || r.credential_version < 1
        || r.sequence < 1
        || r.issued_at.abs_diff(now) > 90
        || r.method != method
        || r.path != path
        || path != format!("/v1/gpu/commands/{action}")
    {
        return fail(StatusCode::FORBIDDEN, "INVALID_COMMAND_BINDING");
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
    let result=match rpc(c,"school_gpu_diagnostic_device",json!({"p_node_id":r.node_id,"p_version":r.credential_version,"p_sequence":r.sequence,"p_issued_at":r.issued_at,"p_action":action,"p_payload":r.payload}),None,true).await{Ok(v)=>v,Err(e)=>return fail(e,"DIAGNOSTIC_REQUEST_DENIED")};
    let command = if result["command"].is_null() {
        None
    } else {
        match serde_json::from_value(result["command"].clone()) {
            Ok(v) => Some(v),
            Err(_) => return fail(StatusCode::BAD_GATEWAY, "INVALID_LEASE_RESPONSE"),
        }
    };
    let envelope = sign_envelope(
        CommandEnvelope {
            protocol: 2,
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
    );
    ([("Cache-Control", "private, no-store")], Json(envelope)).into_response()
}
