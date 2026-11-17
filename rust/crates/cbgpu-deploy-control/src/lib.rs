//! Private native deployment control candidate. No automatic migration/import or live activation.
pub mod api;
pub mod authority;
pub mod config;
pub mod crypto;
pub mod importer;
pub mod legacy_fence;
pub mod log_proxy;
pub mod queue;
pub mod store;
pub mod worker;
use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde_json::json;
#[derive(Debug)]
pub struct Error {
    pub status: u16,
    pub message: String,
}
pub type Result<T> = std::result::Result<T, Error>;
pub fn fail(message: &str, status: u16) -> Error {
    Error {
        status,
        message: message.into(),
    }
}
impl From<sqlx::Error> for Error {
    fn from(error: sqlx::Error) -> Self {
        if error.as_database_error().and_then(|e| e.code()).as_deref() == Some("23505") {
            fail("Name or deployment already exists", 409)
        } else {
            fail("Deployment database unavailable", 503)
        }
    }
}
impl From<std::io::Error> for Error {
    fn from(_: std::io::Error) -> Self {
        fail("Private deployment storage unavailable", 503)
    }
}
impl From<serde_json::Error> for Error {
    fn from(_: serde_json::Error) -> Self {
        fail("Invalid deployment data", 400)
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
            [
                ("cache-control", "no-store"),
                ("x-content-type-options", "nosniff"),
            ],
            Json(json!({"message":self.message})),
        )
            .into_response()
    }
}
pub fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}
pub fn valid_id(value: &str) -> bool {
    uuid::Uuid::parse_str(value).is_ok()
}

pub mod public;

pub mod tenant_proxy;

pub mod reverse;
