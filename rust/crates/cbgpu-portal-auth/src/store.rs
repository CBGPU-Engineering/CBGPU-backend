//! Session persistence only. No hosted user/profile/project writes.
use crate::{Result, SessionRow, fail};
use sqlx::{PgPool, Row};
#[derive(Clone)]
pub struct Store {
    pub pool: PgPool,
}
impl Store {
    pub async fn bootstrap(&self) -> Result<()> {
        let hosted: bool = sqlx::query_scalar("SELECT to_regclass('auth.users') IS NOT NULL")
            .fetch_one(&self.pool)
            .await
            .map_err(|_| fail("PORTAL_STORE_UNAVAILABLE", 503))?;
        if hosted {
            return Err(fail("DEDICATED_RUNTIME_DATABASE_REQUIRED", 409));
        }
        sqlx::raw_sql(include_str!("../sql/store.sql"))
            .execute(&self.pool)
            .await
            .map_err(|_| fail("PORTAL_STORE_BOOTSTRAP_FAILED", 503))?;
        Ok(())
    }
    pub async fn get(&self, hash: &str) -> Result<Option<SessionRow>> {
        let r=sqlx::query("SELECT token_hash,csrf,expires,user_id,email,auth_version,auth_session_id,auth_tokens FROM portal_auth.sessions WHERE scope='developer' AND token_hash=$1").bind(hash).fetch_optional(&self.pool).await.map_err(|_|fail("PORTAL_STORE_UNAVAILABLE",503))?;
        r.map(|r| {
            Ok(SessionRow {
                token_hash: r.try_get("token_hash")?,
                csrf: r.try_get("csrf")?,
                expires: r.try_get("expires")?,
                user_id: r.try_get("user_id")?,
                email: r.try_get("email")?,
                auth_version: r.try_get("auth_version")?,
                auth_session_id: r.try_get("auth_session_id")?,
                auth_tokens: r.try_get("auth_tokens")?,
            })
        })
        .transpose()
        .map_err(|_: sqlx::Error| fail("PORTAL_STORE_INVALID", 503))
    }
    pub async fn insert(
        &self,
        row: &SessionRow,
        previous: Option<&str>,
        failure_keys: &[String],
    ) -> Result<()> {
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|_| fail("PORTAL_STORE_UNAVAILABLE", 503))?;
        if let Some(previous) = previous {
            sqlx::query(
                "DELETE FROM portal_auth.sessions WHERE scope='developer' AND token_hash=$1",
            )
            .bind(previous)
            .execute(&mut *tx)
            .await
            .map_err(|_| fail("PORTAL_STORE_UNAVAILABLE", 503))?;
        }
        sqlx::query("INSERT INTO portal_auth.sessions(scope,token_hash,csrf,expires,user_id,email,auth_version,auth_session_id,auth_tokens) VALUES('developer',$1,$2,$3,$4,$5,$6,$7,$8)").bind(&row.token_hash).bind(&row.csrf).bind(row.expires).bind(row.user_id).bind(&row.email).bind(&row.auth_version).bind(row.auth_session_id).bind(&row.auth_tokens).execute(&mut *tx).await.map_err(|_|fail("PORTAL_STORE_UNAVAILABLE",503))?;
        sqlx::query("INSERT INTO portal_auth.audit(actor_id,action,outcome) VALUES($1,'login.succeeded','success')").bind(row.user_id).execute(&mut *tx).await.map_err(|_|fail("PORTAL_AUDIT_UNAVAILABLE",503))?;
        sqlx::query("DELETE FROM portal_auth.failures WHERE scope='developer' AND key=ANY($1)")
            .bind(failure_keys)
            .execute(&mut *tx)
            .await
            .map_err(|_| fail("PORTAL_STORE_UNAVAILABLE", 503))?;
        tx.commit()
            .await
            .map_err(|_| fail("PORTAL_STORE_UNAVAILABLE", 503))
    }
    pub async fn revoke(&self, row: &SessionRow) -> Result<()> {
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|_| fail("PORTAL_STORE_UNAVAILABLE", 503))?;
        sqlx::query("DELETE FROM portal_auth.sessions WHERE scope='developer' AND token_hash=$1")
            .bind(&row.token_hash)
            .execute(&mut *tx)
            .await
            .map_err(|_| fail("PORTAL_STORE_UNAVAILABLE", 503))?;
        sqlx::query(
            "INSERT INTO portal_auth.audit(actor_id,action,outcome) VALUES($1,'logout','success')",
        )
        .bind(row.user_id)
        .execute(&mut *tx)
        .await
        .map_err(|_| fail("PORTAL_AUDIT_UNAVAILABLE", 503))?;
        tx.commit()
            .await
            .map_err(|_| fail("PORTAL_STORE_UNAVAILABLE", 503))
    }
    pub async fn delete(&self, hash: &str) -> Result<()> {
        sqlx::query("DELETE FROM portal_auth.sessions WHERE scope='developer' AND token_hash=$1")
            .bind(hash)
            .execute(&self.pool)
            .await
            .map_err(|_| fail("PORTAL_STORE_UNAVAILABLE", 503))?;
        Ok(())
    }
    pub async fn audit(&self, id: Option<uuid::Uuid>, action: &str, outcome: &str) -> Result<()> {
        sqlx::query("INSERT INTO portal_auth.audit(actor_id,action,outcome) VALUES($1,$2,$3)")
            .bind(id)
            .bind(action)
            .bind(outcome)
            .execute(&self.pool)
            .await
            .map_err(|_| fail("PORTAL_AUDIT_UNAVAILABLE", 503))?;
        Ok(())
    }
    pub async fn attempt(&self, key: &str, maximum: i32) -> Result<()> {
        let now = crate::now_ms();
        let count:i32=sqlx::query_scalar("INSERT INTO portal_auth.failures(scope,key,count,expires) VALUES('developer',$1,1,$2) ON CONFLICT(scope,key) DO UPDATE SET count=CASE WHEN portal_auth.failures.expires>$3 THEN LEAST(portal_auth.failures.count,9999)+1 ELSE 1 END,expires=CASE WHEN portal_auth.failures.expires>$3 THEN portal_auth.failures.expires ELSE $2 END RETURNING count").bind(key).bind(now+900_000).bind(now).fetch_one(&self.pool).await.map_err(|_|fail("PORTAL_STORE_UNAVAILABLE",503))?;
        if count > maximum {
            Err(fail("TOO_MANY_ATTEMPTS", 429))
        } else {
            Ok(())
        }
    }
    pub async fn clear_attempts(&self, keys: &[String]) -> Result<()> {
        sqlx::query("DELETE FROM portal_auth.failures WHERE scope='developer' AND key=ANY($1)")
            .bind(keys)
            .execute(&self.pool)
            .await
            .map_err(|_| fail("PORTAL_STORE_UNAVAILABLE", 503))?;
        Ok(())
    }
}
