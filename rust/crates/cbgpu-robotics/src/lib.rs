//! Private bounded robotics planning and owner-scoped remote robot persistence.
//! This crate issues validated proposals/goals; it never executes motor commands or contacts devices.
pub mod import;
pub mod planning;
pub mod protocol;
pub mod remote;
pub mod remote_protocol;
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Failure {
    pub message: String,
    pub status: u16,
}
impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for Failure {}
pub type Result<T> = std::result::Result<T, Failure>;
pub fn fail(message: impl Into<String>, status: u16) -> Failure {
    Failure {
        message: message.into(),
        status,
    }
}
pub fn require(condition: bool, message: &str) -> Result<()> {
    if condition {
        Ok(())
    } else {
        Err(fail(message, 400))
    }
}
pub fn now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}
pub fn valid_id(value: &str) -> bool {
    uuid::Uuid::parse_str(value).is_ok_and(|id| id.to_string() == value)
}
pub const SCHEMA: &str = include_str!("schema.sql");
pub async fn migrate(pool: &sqlx::PgPool) -> Result<()> {
    let mut tx = pool.begin().await.map_err(db_error)?;
    sqlx::raw_sql(SCHEMA)
        .execute(&mut *tx)
        .await
        .map_err(db_error)?;
    tx.commit().await.map_err(db_error)?;
    Ok(())
}
pub(crate) fn db_error(_: sqlx::Error) -> Failure {
    fail("Robotics storage is temporarily unavailable", 503)
}
