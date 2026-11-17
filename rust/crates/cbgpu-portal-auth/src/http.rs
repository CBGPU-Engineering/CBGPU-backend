//! Existing auth endpoints only; operational APIs and current terminal sockets remain untouched.
use crate::{
    Result,
    core::{Authenticated, Core},
    crypto::{COOKIE, cookie, csrf},
    fail,
};
use axum::{
    Json, Router,
    body::to_bytes,
    extract::{Request, State},
    http::{HeaderMap, Method, StatusCode, header},
    response::{IntoResponse, Response},
};
use serde_json::{Value, json};
use std::sync::Arc;
pub fn router(core: Arc<Core>) -> Router {
    Router::new().fallback(handle).with_state(core)
}
fn reply(status: u16, value: Value) -> Response {
    let mut r = (
        StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
        Json(value),
    )
        .into_response();
    for (name, value) in [
        ("Cache-Control", "no-store"),
        ("X-Content-Type-Options", "nosniff"),
        ("X-Frame-Options", "DENY"),
        ("Referrer-Policy", "no-referrer"),
        ("X-Robots-Tag", "noindex, nofollow, noarchive"),
        (
            "Content-Security-Policy",
            "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; connect-src 'self'; object-src 'none'; base-uri 'none'; frame-ancestors 'none'",
        ),
    ] {
        r.headers_mut().insert(
            axum::http::HeaderName::from_bytes(name.as_bytes()).unwrap(),
            value.parse().unwrap(),
        );
    }
    r
}
fn text<'a>(h: &'a HeaderMap, key: &str) -> &'a str {
    h.get(key).and_then(|v| v.to_str().ok()).unwrap_or("")
}
pub fn allowed_host(h: &HeaderMap) -> bool {
    matches!(
        text(h, "host").split(':').next().unwrap_or(""),
        "developers.cbgpu.com" | "admin.cbgpu.com" | "localhost" | "127.0.0.1"
    )
}
pub fn origin(h: &HeaderMap) -> Result<()> {
    if matches!(
        text(h, "origin"),
        "https://developers.cbgpu.com"
            | "https://admin.cbgpu.com"
            | "http://127.0.0.1:3400"
            | "http://localhost:3400"
    ) {
        Ok(())
    } else {
        Err(fail("REQUEST_ORIGIN_NOT_ALLOWED", 403))
    }
}
fn info(auth: &Authenticated) -> Value {
    json!({"authenticated":true,"csrf_token":auth.row.csrf,"user":auth.user})
}
fn set_cookie(response: &mut Response, value: String) -> Result<()> {
    response.headers_mut().append(
        header::SET_COOKIE,
        value
            .parse()
            .map_err(|_| fail("COOKIE_ENCODING_FAILED", 503))?,
    );
    Ok(())
}
async fn handle(State(core): State<Arc<Core>>, request: Request) -> Response {
    let result = process(core, request).await;
    match result {
        Ok(response) => response,
        Err(e) => reply(e.status, json!({"error":e.code})),
    }
}
async fn process(core: Arc<Core>, request: Request) -> Result<Response> {
    if !allowed_host(request.headers()) {
        return Err(fail("HOST_NOT_ALLOWED", 403));
    }
    let path = request.uri().path();
    let method = request.method();
    let admin = text(request.headers(), "host").split(':').next() == Some("admin.cbgpu.com");
    let route = if admin && path.starts_with("/portal/api/") {
        &path[7..]
    } else {
        path
    };
    if route == "/health" && method == Method::GET {
        return Ok(reply(
            200,
            json!({"ok":true,"implementation":"native-rust","scope":"portal-auth-only","terminal_handoff":false}),
        ));
    }
    if request.uri().query().is_some() {
        return Err(fail("UNEXPECTED_QUERY", 400));
    }
    if route == "/api/session" && method == Method::GET {
        let auth = core.authenticate(text(request.headers(), "cookie")).await?;
        return Ok(reply(
            200,
            auth.as_ref()
                .map(info)
                .unwrap_or(json!({"authenticated":false})),
        ));
    }
    if matches!(route, "/api/login/system" | "/api/login/cancel") || route.starts_with("/api/mfa/")
    {
        return Err(fail("ADDITIONAL_SIGN_IN_STEP_REMOVED", 410));
    }
    if route == "/api/login" && method == Method::POST {
        origin(request.headers())?;
        let ip = text(request.headers(), "cf-connecting-ip")
            .chars()
            .take(100)
            .collect::<String>();
        let old = text(request.headers(), "cookie").to_string();
        let bytes = to_bytes(request.into_body(), 4096)
            .await
            .map_err(|_| fail("REQUEST_BODY_TOO_LARGE", 413))?;
        let value: Value = serde_json::from_slice(&bytes).map_err(|_| fail("INVALID_JSON", 400))?;
        let login = core
            .login(
                value["email"].as_str().unwrap_or(""),
                value["password"].as_str().unwrap_or(""),
                if ip.is_empty() { "local" } else { &ip },
                &old,
            )
            .await;
        let (auth, raw) = match login {
            Ok(v) => v,
            Err(error) => {
                core.store
                    .audit(
                        None,
                        "login.account_failed",
                        if error.status >= 500 {
                            "failed"
                        } else {
                            "denied"
                        },
                    )
                    .await?;
                return Err(error);
            }
        };
        let mut response = reply(200, info(&auth));
        set_cookie(&mut response, cookie(COOKIE, &raw, 28800))?;
        set_cookie(&mut response, cookie(&format!("{COOKIE}-pending"), "", 0))?;
        return Ok(response);
    }
    if route == "/api/logout" && method == Method::POST {
        origin(request.headers())?;
        let auth = core
            .authenticate(text(request.headers(), "cookie"))
            .await?
            .ok_or_else(|| fail("PLEASE_SIGN_IN", 401))?;
        csrf(&auth.row.csrf, text(request.headers(), "x-csrf-token"))?;
        core.logout(&auth).await?;
        let mut response = reply(200, json!({"ok":true}));
        set_cookie(&mut response, cookie(COOKIE, "", 0))?;
        set_cookie(&mut response, cookie(&format!("{COOKIE}-pending"), "", 0))?;
        return Ok(response);
    }
    if route == "/portal/session" && admin && method == Method::GET {
        let auth = core
            .authenticate(text(request.headers(), "cookie"))
            .await?
            .ok_or_else(|| fail("PLEASE_SIGN_IN", 401))?;
        return Ok(reply(
            200,
            json!({"user_id":auth.row.user_id,"auth_session_id":auth.row.auth_session_id,"csrf_token":auth.row.csrf,"session":core.native_session(&auth)}),
        ));
    }
    Err(fail("NOT_IMPLEMENTED_BY_AUTH_CANDIDATE", 404))
}
