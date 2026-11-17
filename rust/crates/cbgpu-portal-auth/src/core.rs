use crate::{
    Member, Principal, Result, SessionRow, Tokens, User,
    authority::{GoTrue, Oracle},
    crypto::{Crypto, claims, token},
    fail, now_ms, roster_member,
    store::Store,
    validate_roster,
};
use rand::RngCore;
use serde::Deserialize;
use std::{path::PathBuf, sync::Arc};
#[derive(Clone)]
pub enum Roster {
    Fixed(Vec<Member>),
    File(PathBuf),
}
impl Roster {
    pub fn read(&self) -> Result<Vec<Member>> {
        let items = match self {
            Self::Fixed(v) => v.clone(),
            Self::File(path) => {
                #[derive(Deserialize)]
                struct Settings {
                    developers: Vec<Member>,
                }
                let meta =
                    std::fs::symlink_metadata(path).map_err(|_| fail("ROSTER_UNAVAILABLE", 503))?;
                if !meta.is_file() || meta.len() > 32768 {
                    return Err(fail("ROSTER_UNAVAILABLE", 503));
                }
                let data = std::fs::read(path).map_err(|_| fail("ROSTER_UNAVAILABLE", 503))?;
                serde_json::from_slice::<Settings>(&data)
                    .map_err(|_| fail("ROSTER_INVALID", 503))?
                    .developers
            }
        };
        validate_roster(&items)?;
        Ok(items)
    }
}
pub struct Core {
    pub store: Store,
    pub crypto: Crypto,
    pub roster: Roster,
    pub oracle: Oracle,
    pub gotrue: GoTrue,
    pub scoped: bool,
    pub revoked: tokio::sync::broadcast::Sender<String>,
}
pub struct Authenticated {
    pub row: SessionRow,
    pub user: User,
}
impl Core {
    pub async fn authenticate_hash(&self, hash: &str) -> Result<Option<Authenticated>> {
        if !token(hash) {
            return Ok(None);
        }
        let Some(row) = self.store.get(hash).await? else {
            return Ok(None);
        };
        if row.expires <= now_ms() {
            return Ok(None);
        }
        if self.crypto.row_tokens(&row).is_err() || !token(&row.csrf) {
            return Ok(None);
        }
        let roster = self.roster.read()?;
        let Some(member) = roster_member(&roster, &row.email) else {
            return Ok(None);
        };
        if member.id != row.user_id {
            return Ok(None);
        }
        let Some(principal) = self
            .oracle
            .principal(row.user_id, row.auth_session_id)
            .await?
        else {
            return Ok(None);
        };
        if !eligible(&principal, member, self.scoped) {
            return Ok(None);
        }
        Ok(Some(Authenticated {
            user: User {
                id: principal.id,
                email: principal.email,
                name: principal.name,
                verification_method: "cbgpu_account",
            },
            row,
        }))
    }
    pub async fn authenticate(&self, cookie: &str) -> Result<Option<Authenticated>> {
        let Some(raw) = crate::crypto::read_cookie(cookie) else {
            return Ok(None);
        };
        self.authenticate_hash(&self.crypto.hash(raw)).await
    }
    pub async fn login(
        &self,
        email: &str,
        password: &str,
        ip: &str,
        old_cookie: &str,
    ) -> Result<(Authenticated, String)> {
        let email = email.trim().to_lowercase();
        let keys = [
            self.crypto.hash(&format!("{ip}:account-global")),
            self.crypto.hash(&format!("{ip}:account:{email}")),
        ];
        self.store.attempt(&keys[0], 30).await?;
        self.store.attempt(&keys[1], 7).await?;
        let roster = self.roster.read()?;
        let Some(member) = roster_member(&roster, &email) else {
            return Err(fail("ACCOUNT_CREDENTIALS_OR_ACCESS_UNAVAILABLE", 401));
        };
        if password.is_empty() || password.encode_utf16().count() > 256 || email.len() > 254 {
            return Err(fail("ACCOUNT_CREDENTIALS_OR_ACCESS_UNAVAILABLE", 401));
        }
        let (id, tokens) = self.gotrue.login(&email, password).await?;
        let claims = claims(&tokens.access_token)?;
        if id != member.id || claims.sub != id || claims.exp <= now_ms() / 1000 {
            return Err(fail("ACCOUNT_IDENTITY_MISMATCH", 401));
        }
        let Some(principal) = self.oracle.principal(id, claims.session_id).await? else {
            return Err(fail("ACCOUNT_SESSION_REVOKED", 401));
        };
        if !eligible(&principal, member, self.scoped) {
            return Err(fail("ACCOUNT_CREDENTIALS_OR_ACCESS_UNAVAILABLE", 401));
        }
        let random = || {
            let mut b = [0u8; 32];
            rand::thread_rng().fill_bytes(&mut b);
            b.iter().map(|v| format!("{v:02x}")).collect::<String>()
        };
        let raw = random();
        let row = SessionRow {
            token_hash: self.crypto.hash(&raw),
            csrf: random(),
            expires: now_ms() + 8 * 3_600_000,
            user_id: id,
            email: email.clone(),
            auth_version: self.crypto.version(id),
            auth_session_id: claims.session_id,
            auth_tokens: self.crypto.seal(&tokens)?,
        };
        let previous = crate::crypto::read_cookie(old_cookie).map(|v| self.crypto.hash(v));
        self.store.insert(&row, previous.as_deref(), &keys).await?;
        if let Some(hash) = previous {
            let _ = self.revoked.send(hash);
        }
        Ok((
            Authenticated {
                row,
                user: User {
                    id,
                    email: principal.email,
                    name: principal.name,
                    verification_method: "cbgpu_account",
                },
            },
            raw,
        ))
    }
    pub async fn logout(&self, auth: &Authenticated) -> Result<()> {
        self.store.revoke(&auth.row).await?;
        let _ = self.revoked.send(auth.row.token_hash.clone());
        Ok(())
    }
    pub fn native_session(&self, auth: &Authenticated) -> Option<Tokens> {
        let (t, c) = self.crypto.row_tokens(&auth.row).ok()?;
        if c.exp.saturating_mul(1000) > now_ms() + 30_000 {
            Some(t)
        } else {
            None
        }
    }
    /// A future Rust terminal can use this without handing off current Node sockets.
    /// A four-second tick plus one-second authority deadline bounds the normal
    /// check cycle to five seconds; errors/timeouts fail closed.
    pub async fn monitor(self: Arc<Self>, hash: String, stop: tokio::sync::watch::Sender<bool>) {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(4));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut revoked = self.revoked.subscribe();
        loop {
            tokio::select! {result=async{interval.tick().await;tokio::time::timeout(std::time::Duration::from_secs(1),self.authenticate_hash(&hash)).await}=>{if !matches!(result,Ok(Ok(Some(_)))){let _=stop.send(true);break}},event=revoked.recv()=>{if event.is_err()||event.ok().as_deref()==Some(&hash){let _=stop.send(true);break}},_=stop.closed()=>break}
        }
    }
}
pub fn eligible(principal: &Principal, member: &Member, scoped: bool) -> bool {
    principal.valid
        && principal.id == member.id
        && principal.email.eq_ignore_ascii_case(&member.email)
        && principal.is_admin
        && (scoped || principal.legacy_approved)
}
