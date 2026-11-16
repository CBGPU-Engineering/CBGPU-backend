//! Separate write-enabled EXECUTE-only pool. Never reuse or weaken the JWT Auth reader pool.
use crate::{MintRequest, Persistence, RegistrationInput, Result, TokenHash, Verified, fail};
use serde_json::{Value, json};
use sqlx::{PgPool, Postgres, Transaction};
pub struct Store {
    pub pool: PgPool,
}
fn error(error: sqlx::Error) -> crate::Error {
    if let Some(error) = error.as_database_error() {
        let code = error.code().unwrap_or_default();
        match code.as_ref() {
            "PT401" => {
                return fail(
                    401,
                    "invalid_contest_token",
                    "Contest credential expired, revoked, or its parent session is unavailable",
                );
            }
            "PT403" => {
                return fail(
                    403,
                    "token_scope_denied",
                    "This token or account cannot perform that action on this contest",
                );
            }
            "PT404" => {
                return fail(
                    404,
                    "not_found",
                    "Contest, token or own registration not found",
                );
            }
            "PT429" => {
                return fail(
                    429,
                    "token_limit_reached",
                    "Maximum20 active contest tokens per account",
                );
            }
            "PT400" => {
                return fail(
                    400,
                    "invalid_token_request",
                    "Invalid contest-token request",
                );
            }
            _ => {}
        }
        return cbgpu_contests_api::backend::database_error(
            400,
            &json!({"code":code,"message":error.message()}),
        );
    }
    fail(
        503,
        "token_backend_unavailable",
        "Contest token database unavailable",
    )
}
impl Store {
    async fn jwt<'a>(&'a self, actor: &Verified) -> Result<Transaction<'a, Postgres>> {
        let mut tx = self.pool.begin().await.map_err(error)?;
        sqlx::query("SELECT set_config('request.jwt.claims',$1,true),set_config('request.jwt.claim.sub',$2,true)").bind(json!({"role":"authenticated","sub":actor.subject,"session_id":actor.session}).to_string()).bind(actor.subject.to_string()).execute(&mut*tx).await.map_err(error)?;
        Ok(tx)
    }
}
#[async_trait::async_trait]
impl Persistence for Store {
    async fn mint(&self, actor: &Verified, hash: &TokenHash, input: &MintRequest) -> Result<Value> {
        input.validate()?;
        let mut tx = self.jwt(actor).await?;
        let row = sqlx::query_scalar("SELECT contest_api.mint($1,$2,$3,$4,$5)")
            .bind(hash.bytes())
            .bind(input.name.trim())
            .bind(&input.scopes)
            .bind(&input.allowed_contest_ids)
            .bind(i32::from(input.expires_in_days))
            .fetch_one(&mut *tx)
            .await
            .map_err(error)?;
        tx.commit().await.map_err(error)?;
        Ok(row)
    }
    async fn list(&self, actor: &Verified) -> Result<Value> {
        let mut tx = self.jwt(actor).await?;
        let row = sqlx::query_scalar("SELECT contest_api.list_owned()")
            .fetch_one(&mut *tx)
            .await
            .map_err(error)?;
        tx.commit().await.map_err(error)?;
        Ok(row)
    }
    async fn revoke(&self, actor: &Verified, id: uuid::Uuid) -> Result<bool> {
        let mut tx = self.jwt(actor).await?;
        let ok = sqlx::query_scalar("SELECT contest_api.revoke_owned($1)")
            .bind(id)
            .fetch_one(&mut *tx)
            .await
            .map_err(error)?;
        tx.commit().await.map_err(error)?;
        Ok(ok)
    }
    async fn read(&self, hash: &TokenHash, id: Option<uuid::Uuid>, own: bool) -> Result<Value> {
        let mut tx = self.pool.begin().await.map_err(error)?;
        let row = sqlx::query_scalar("SELECT contest_api.read_allowed($1,$2,$3)")
            .bind(hash.bytes())
            .bind(id)
            .bind(own)
            .fetch_one(&mut *tx)
            .await
            .map_err(error)?;
        tx.commit().await.map_err(error)?;
        Ok(row)
    }
    async fn register(
        &self,
        hash: &TokenHash,
        id: uuid::Uuid,
        input: &RegistrationInput,
    ) -> Result<Value> {
        input.validate()?;
        let mut tx = self.pool.begin().await.map_err(error)?;
        let row = sqlx::query_scalar("SELECT contest_api.register_allowed($1,$2,$3,$4,$5,$6,$7)")
            .bind(hash.bytes())
            .bind(id)
            .bind(input.attendee_name.trim())
            .bind(&input.answers)
            .bind(&input.kind)
            .bind(input.team_name.as_deref().map(str::trim))
            .bind(input.join_code)
            .fetch_one(&mut *tx)
            .await
            .map_err(error)?;
        tx.commit().await.map_err(error)?;
        Ok(row)
    }
    async fn cancel(&self, hash: &TokenHash, id: uuid::Uuid) -> Result<Value> {
        let mut tx = self.pool.begin().await.map_err(error)?;
        let row = sqlx::query_scalar("SELECT contest_api.cancel_allowed($1,$2)")
            .bind(hash.bytes())
            .bind(id)
            .fetch_one(&mut *tx)
            .await
            .map_err(error)?;
        tx.commit().await.map_err(error)?;
        Ok(row)
    }
}

/// Fail startup if the execution connection accidentally uses a broad role or the Auth reader.
pub async fn verify_executor(pool: &PgPool) -> Result<()> {
    let readonly: String = sqlx::query_scalar("SHOW default_transaction_read_only")
        .fetch_one(pool)
        .await
        .map_err(error)?;
    if readonly != "off" {
        return Err(fail(
            503,
            "configuration",
            "Token executor must use its own write-enabled EXECUTE-only login",
        ));
    }
    let broad:bool=sqlx::query_scalar("SELECT r.rolsuper OR r.rolbypassrls OR r.rolcreatedb OR r.rolcreaterole OR pg_has_role(current_user,'authenticated','MEMBER') OR pg_has_role(current_user,'cbgpu_contest_policy_reader','MEMBER') OR EXISTS(SELECT 1 FROM pg_catalog.pg_class c JOIN pg_catalog.pg_namespace n ON n.oid=c.relnamespace WHERE((n.nspname='auth'AND c.relname IN('users','sessions'))OR(n.nspname='public'AND c.relname IN('profiles','contests','event_registrations'))OR(n.nspname='contest_api'AND c.relname='tokens'))AND(has_any_column_privilege(current_user,c.oid,'SELECT,INSERT,UPDATE')OR has_table_privilege(current_user,c.oid,'DELETE,TRUNCATE'))) OR EXISTS(SELECT 1 FROM pg_proc p JOIN pg_namespace n ON n.oid=p.pronamespace WHERE((n.nspname='contest_api'AND left(p.proname::text,1)='_')OR(n.nspname='public'AND p.proname='register_contest_entry'))AND has_function_privilege(current_user,p.oid,'EXECUTE')) FROM pg_roles r WHERE r.rolname=current_user").fetch_one(pool).await.map_err(error)?;
    if broad {
        return Err(fail(
            503,
            "configuration",
            "Token execution login has unexpected raw-data, helper, role or elevated privileges",
        ));
    }
    let grants:bool=sqlx::query_scalar("SELECT count(*)=6 AND coalesce(bool_and(has_function_privilege(current_user,p.oid,'EXECUTE')),false)FROM pg_proc p WHERE p.oid=ANY(ARRAY[to_regprocedure('contest_api.mint(bytea,text,text[],uuid[],integer)'),to_regprocedure('contest_api.list_owned()'),to_regprocedure('contest_api.revoke_owned(uuid)'),to_regprocedure('contest_api.read_allowed(bytea,uuid,boolean)'),to_regprocedure('contest_api.register_allowed(bytea,uuid,text,jsonb,text,text,uuid)'),to_regprocedure('contest_api.cancel_allowed(bytea,uuid)')]::oid[])").fetch_one(pool).await.map_err(error)?;
    if !grants {
        return Err(fail(
            503,
            "configuration",
            "All six reviewed token function grants are required",
        ));
    }
    let policy_reader:bool=sqlx::query_scalar("SELECT count(*)=2 AND coalesce(bool_and(p.proowner=r.oid AND p.prosecdef AND p.provolatile='s' AND p.pronargs=1 AND p.proargtypes[0]='uuid'::regtype AND coalesce(p.proconfig @> ARRAY['row_security=on','search_path=\"\"']::text[],false)AND NOT r.rolcanlogin AND NOT r.rolsuper AND NOT r.rolbypassrls AND r.rolinherit AND pg_has_role(r.oid,'authenticated','USAGE')AND NOT EXISTS(SELECT 1 FROM pg_class c WHERE c.relowner=r.oid)AND NOT EXISTS(SELECT 1 FROM pg_namespace n WHERE n.nspowner=r.oid)),false)FROM pg_proc p JOIN pg_namespace n ON n.oid=p.pronamespace JOIN pg_roles r ON r.rolname='cbgpu_contest_policy_reader'WHERE n.nspname='contest_api'AND p.proname IN('_policy_visible','_policy_contest')").fetch_one(pool).await.map_err(error)?;
    if !policy_reader {
        return Err(fail(
            503,
            "configuration",
            "Policy reader ownership, inherited RLS or function configuration has changed",
        ));
    }
    Ok(())
}
