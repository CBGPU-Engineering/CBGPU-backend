//! Credential selection for future native portal proxy integration, not signature verification.
use crate::{Result, Tokens, core::Authenticated, crypto::claims, fail, now_ms};
/// Caller JWT claims are used ONLY to match an independently authorized portal
/// cookie. GoTrue/Kong at the actual upstream MUST verify the signature. Never
/// use this decoded payload as proof of identity or role authority.
pub fn bound_bearer(
    caller: Option<&str>,
    auth: &Authenticated,
    cached: Option<Tokens>,
) -> Result<Option<String>> {
    if let Some(header) = caller {
        let token = header
            .strip_prefix("Bearer ")
            .filter(|v| !v.is_empty() && v.len() <= 8192)
            .ok_or_else(|| fail("INVALID_CALLER_BEARER", 401))?;
        let c = claims(token)?;
        if c.sub != auth.row.user_id
            || c.session_id != auth.row.auth_session_id
            || c.exp.saturating_mul(1000) <= now_ms()
        {
            return Err(fail("CALLER_SESSION_MISMATCH", 401));
        }
        return Ok(Some(header.into()));
    }
    if let Some(tokens) = cached {
        let c = claims(&tokens.access_token)?;
        if c.sub != auth.row.user_id
            || c.session_id != auth.row.auth_session_id
            || c.exp.saturating_mul(1000) <= now_ms() + 30_000
        {
            return Err(fail("CACHED_SESSION_EXPIRED", 401));
        }
        return Ok(Some(format!("Bearer {}", tokens.access_token)));
    }
    Ok(None)
}
