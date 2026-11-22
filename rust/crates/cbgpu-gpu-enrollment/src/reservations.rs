//! Optional trusted transaction adapter. Never creates bookings or touches an executor.
use crate::{Config, bearer, fail};
use axum::{
    Json,
    body::to_bytes,
    extract::Request,
    http::{Method, StatusCode},
    response::{IntoResponse, Response},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::{PgPool, Postgres, Row, Transaction};
use uuid::Uuid;

#[derive(Clone, Debug)]
pub enum Scope {
    Legacy,
    School(String),
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Extension {
    pub ends_at: DateTime<Utc>,
    pub expected_ends_at: DateTime<Utc>,
}
#[derive(Clone, Copy)]
pub struct Caller {
    pub user: Uuid,
    pub session: Uuid,
    pub expires: i64,
}
#[derive(Debug)]
pub struct Error(pub StatusCode, pub &'static str);
type Result<T> = std::result::Result<T, Error>;
impl From<sqlx::Error> for Error {
    fn from(error: sqlx::Error) -> Self {
        match error.as_database_error().and_then(|e| e.code()).as_deref() {
            Some("23P01") => Self(StatusCode::CONFLICT, "RESERVATION_CONFLICT"),
            Some("55P03" | "57014" | "40P01") => {
                Self(StatusCode::SERVICE_UNAVAILABLE, "RESERVATION_BUSY")
            }
            _ => Self(StatusCode::SERVICE_UNAVAILABLE, "EXTENSION_UNAVAILABLE"),
        }
    }
}
fn conflict(code: &'static str) -> Error {
    Error(StatusCode::CONFLICT, code)
}

pub fn is_path(path: &str) -> bool {
    (path.starts_with("/v1/gpu/legacy/reservations/") || path.starts_with("/v1/schools/"))
        && (path.ends_with("/extension") || path.ends_with("/extend"))
}
fn route(path: &str, method: &Method) -> Option<(Scope, Uuid, bool)> {
    let p = path.split('/').collect::<Vec<_>>();
    let (scope, id, op) = match p.as_slice() {
        ["", "v1", "gpu", "legacy", "reservations", id, op] => (Scope::Legacy, *id, *op),
        ["", "v1", "schools", school, "gpu", "reservations", id, op]
            if !school.is_empty()
                && school.len() <= 100
                && school
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b)) =>
        {
            (Scope::School((*school).to_owned()), *id, *op)
        }
        _ => return None,
    };
    let write = match (method, op) {
        (&Method::GET, "extension") => false,
        (&Method::POST, "extend") => true,
        _ => return None,
    };
    Some((scope, Uuid::parse_str(id).ok()?, write))
}

// Claims are used only AFTER GoTrue validates this exact token and returns the same UID.
fn verified_claims(token: &str, verified_user: Uuid) -> Option<Caller> {
    let fields = token
        .split_whitespace()
        .nth(1)?
        .split('.')
        .collect::<Vec<_>>();
    if fields.len() != 3 {
        return None;
    }
    let value: Value = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(fields[1]).ok()?).ok()?;
    if Uuid::parse_str(value["sub"].as_str()?).ok()? != verified_user {
        return None;
    }
    Some(Caller {
        user: verified_user,
        session: Uuid::parse_str(value["session_id"].as_str()?).ok()?,
        expires: value["exp"].as_i64()?,
    })
}
pub async fn handle(c: &Config, request: Request) -> Response {
    let Some((scope, id, write)) = route(request.uri().path(), request.method()) else {
        return fail(StatusCode::NOT_FOUND, "NOT_FOUND");
    };
    let Some(token) = bearer(&request) else {
        return fail(StatusCode::UNAUTHORIZED, "AUTHENTICATION_REQUIRED");
    };
    let Some(pool) = &c.reservations else {
        return fail(StatusCode::SERVICE_UNAVAILABLE, "EXTENSION_UNAVAILABLE");
    };
    let bytes = match to_bytes(request.into_body(), 1024).await {
        Ok(v) => v,
        Err(_) => return fail(StatusCode::BAD_REQUEST, "INVALID_BODY"),
    };
    let change = if write {
        match serde_json::from_slice::<Extension>(&bytes) {
            Ok(v) => Some(v),
            Err(_) => return fail(StatusCode::BAD_REQUEST, "INVALID_BODY"),
        }
    } else if bytes.is_empty() {
        None
    } else {
        return fail(StatusCode::BAD_REQUEST, "INVALID_BODY");
    };
    let response = c
        .client
        .get(format!("{}/auth/v1/user", c.backend_url))
        .header("apikey", &c.anon_key)
        .header("authorization", &token)
        .send()
        .await;
    let mut response = match response {
        Ok(r) if r.status().is_success() => r,
        Ok(r) if r.status().is_server_error() || r.status().as_u16() == 429 => {
            return fail(StatusCode::SERVICE_UNAVAILABLE, "AUTH_UNAVAILABLE");
        }
        Ok(_) => return fail(StatusCode::UNAUTHORIZED, "INVALID_SESSION"),
        Err(_) => return fail(StatusCode::SERVICE_UNAVAILABLE, "AUTH_UNAVAILABLE"),
    };
    let mut body = Vec::new();
    loop {
        match response.chunk().await {
            Ok(Some(chunk)) if body.len() + chunk.len() <= 65536 => body.extend_from_slice(&chunk),
            Ok(None) => break,
            _ => return fail(StatusCode::SERVICE_UNAVAILABLE, "AUTH_UNAVAILABLE"),
        }
    }
    let user = serde_json::from_slice::<Value>(&body)
        .ok()
        .and_then(|v| v["id"].as_str().and_then(|s| Uuid::parse_str(s).ok()));
    let Some(user) = user else {
        return fail(StatusCode::SERVICE_UNAVAILABLE, "AUTH_UNAVAILABLE");
    };
    let Some(caller) = verified_claims(&token, user) else {
        return fail(StatusCode::UNAUTHORIZED, "INVALID_SESSION");
    };
    match tokio::time::timeout(
        std::time::Duration::from_secs(8),
        transact(pool, caller, scope, id, change),
    )
    .await
    {
        Ok(Ok(value)) => (
            StatusCode::OK,
            [("Cache-Control", "private, no-store")],
            Json(value),
        )
            .into_response(),
        Ok(Err(Error(status, code))) => fail(status, code),
        Err(_) => fail(StatusCode::SERVICE_UNAVAILABLE, "RESERVATION_BUSY"),
    }
}

struct Booking {
    starts: DateTime<Utc>,
    ends: DateTime<Utc>,
    active_status: bool,
    gpu: Option<i32>,
    resource: Option<Uuid>,
    has_job: bool,
    ready: bool,
}
async fn identity(tx: &mut Transaction<'_, Postgres>, caller: Caller, scope: &Scope) -> Result<()> {
    let valid:bool=sqlx::query_scalar("SELECT public.cbgpu_personal_project_identity() AND extract(epoch FROM clock_timestamp()) < $1::bigint AND EXISTS(SELECT FROM auth.sessions s WHERE s.id=$2 AND s.user_id=$3 AND (s.not_after IS NULL OR s.not_after>clock_timestamp()))").bind(caller.expires).bind(caller.session).bind(caller.user).fetch_one(&mut **tx).await?;
    if !valid {
        return Err(Error(StatusCode::UNAUTHORIZED, "INVALID_SESSION"));
    }
    let allowed: bool = match scope {
        Scope::Legacy => {
            sqlx::query_scalar("SELECT public.cbgpu_can_use_resource('gpu','cbsh')")
                .fetch_one(&mut **tx)
                .await?
        }
        Scope::School(s) => {
            sqlx::query_scalar("SELECT cbgpu_schools.node_gpu_permission($1,$2)")
                .bind(caller.user)
                .bind(s)
                .fetch_one(&mut **tx)
                .await?
        }
    };
    if !allowed {
        return Err(Error(StatusCode::FORBIDDEN, "GPU_PERMISSION_REQUIRED"));
    }
    Ok(())
}

pub async fn transact(
    pool: &PgPool,
    caller: Caller,
    scope: Scope,
    id: Uuid,
    change: Option<Extension>,
) -> Result<Value> {
    let mut tx = pool.begin().await?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL READ COMMITTED")
        .execute(&mut *tx)
        .await?;
    sqlx::query("SET LOCAL lock_timeout='1500ms'")
        .execute(&mut *tx)
        .await?;
    sqlx::query("SET LOCAL statement_timeout='4s'")
        .execute(&mut *tx)
        .await?;
    sqlx::query("SET LOCAL idle_in_transaction_session_timeout='5s'")
        .execute(&mut *tx)
        .await?;
    sqlx::query("SELECT set_config('request.jwt.claims',$1,true)")
        .bind(
            json!({"sub":caller.user,"session_id":caller.session,"role":"authenticated"})
                .to_string(),
        )
        .execute(&mut *tx)
        .await?;
    identity(&mut tx, caller, &scope).await?;
    if change.is_some() {
        // Unlike an application-only advisory lock, this also fences direct PostgREST writers.
        // EXCLUSIVE also fences SELECT FOR UPDATE used by notebook launch before checking no job.
        sqlx::query(match scope {
            Scope::Legacy => "LOCK TABLE public.reservations IN EXCLUSIVE MODE",
            Scope::School(_) => {
                "LOCK TABLE cbgpu_schools.gpu_resource_reservations IN EXCLUSIVE MODE"
            }
        })
        .execute(&mut *tx)
        .await?;
    }
    let booking=match &scope {
        Scope::Legacy=>sqlx::query("SELECT start_time, end_time, status='approved' AS active_status,gpu_id FROM public.reservations WHERE id=$1 AND user_id=$2").bind(id).bind(caller.user).fetch_optional(&mut *tx).await?.map(|r|Booking {starts:r.get("start_time"),ends:r.get("end_time"),active_status:r.get("active_status"),gpu:Some(r.get("gpu_id")),resource:None,has_job:false,ready:true}),
        Scope::School(s)=>sqlx::query("SELECT b.starts_at,b.ends_at,b.status='RESERVED' AS active_status,b.resource_id,EXISTS(SELECT FROM cbgpu_schools.gpu_notebook_jobs j WHERE j.reservation_id=b.id) AS has_job, cbgpu_schools.gpu_resource_state(r)='READY' AS ready FROM cbgpu_schools.gpu_resource_reservations b JOIN cbgpu_schools.gpu_resources r ON r.id=b.resource_id JOIN cbgpu_schools.gpu_nodes n ON n.id=r.node_id WHERE b.id=$1 AND b.owner_id=$2 AND n.school_id=$3").bind(id).bind(caller.user).bind(s).fetch_optional(&mut *tx).await?.map(|r|Booking{starts:r.get("starts_at"),ends:r.get("ends_at"),active_status:r.get("active_status"),gpu:None,resource:Some(r.get("resource_id")),has_job:r.get("has_job"),ready:r.get("ready")}),
    }.ok_or(Error(StatusCode::NOT_FOUND,"RESERVATION_NOT_FOUND"))?;
    let now: DateTime<Utc> = sqlx::query_scalar("SELECT clock_timestamp()")
        .fetch_one(&mut *tx)
        .await?;
    let mut reason = if booking.ends <= now {
        Some("RESERVATION_EXPIRED")
    } else if !booking.active_status || booking.starts > now {
        Some("RESERVATION_NOT_ACTIVE")
    } else if booking.has_job {
        Some("EXECUTOR_EXTENSION_UNSUPPORTED")
    } else if !booking.ready {
        Some("GPU_RESOURCE_UNAVAILABLE")
    } else {
        None
    };
    let school = match &scope {
        Scope::Legacy => "cbsh",
        Scope::School(s) => s,
    };
    // A null membership end means unbounded; guest approvals always have finite ends.
    let grant_end:Option<DateTime<Utc>>=sqlx::query_scalar("SELECT CASE WHEN bool_or(e IS NULL) THEN NULL ELSE max(e) END FROM (SELECT m.ends_at e FROM cbgpu_schools.memberships m WHERE m.user_id=$1 AND m.school_id=$2 AND m.membership_type IN ('STUDENT','TEACHER') AND cbgpu_schools.active_member($1,$2) AND m.starts_at<=clock_timestamp() UNION ALL SELECT g.ends_at FROM cbgpu_accounts.guest_requests g WHERE g.external_user_id=$1 AND g.school_id=$2 AND g.status='APPROVED' AND 'gpu'=ANY(g.resources) AND g.starts_at<=clock_timestamp() AND g.ends_at>clock_timestamp() AND cbgpu_schools.active_member(g.requester_id,$2) AND EXISTS(SELECT FROM cbgpu_schools.memberships sponsor WHERE sponsor.user_id=g.requester_id AND sponsor.school_id=$2 AND sponsor.membership_type IN ('STUDENT','TEACHER'))) valid_grants").bind(caller.user).bind(school).fetch_one(&mut *tx).await?;
    let next:Option<DateTime<Utc>>=match scope {
        Scope::Legacy=>sqlx::query_scalar("SELECT min(start_time) FROM public.reservations WHERE gpu_id=$1 AND id<>$2 AND status IN ('pending','approved') AND end_time>$3").bind(booking.gpu).bind(id).bind(booking.ends).fetch_one(&mut *tx).await?,
        Scope::School(_)=>sqlx::query_scalar("SELECT min(starts_at) FROM cbgpu_schools.gpu_resource_reservations WHERE resource_id=$1 AND id<>$2 AND status='RESERVED' AND ends_at>$3").bind(booking.resource).bind(id).bind(booking.ends).fetch_one(&mut *tx).await?,
    };
    let cap =
        matches!(scope, Scope::School(_)).then(|| booking.starts + chrono::Duration::hours(4));
    let max_end = [grant_end, next, cap].into_iter().flatten().min();
    if reason.is_none() && max_end.is_some_and(|v| v <= booking.ends) {
        reason = Some(if next.is_some_and(|v| v <= booking.ends) {
            "RESERVATION_CONFLICT"
        } else {
            "EXTENSION_LIMIT"
        });
    }
    if let Some(change) = change {
        if let Some(code) = reason {
            return Err(conflict(code));
        }
        if change.expected_ends_at != booking.ends {
            return Err(conflict("RESERVATION_CHANGED"));
        }
        if change.ends_at <= booking.ends {
            return Err(Error(StatusCode::BAD_REQUEST, "INVALID_WINDOW"));
        }
        if max_end.is_some_and(|end| change.ends_at > end) {
            return Err(conflict(if next.is_some_and(|v| change.ends_at > v) {
                "RESERVATION_CONFLICT"
            } else {
                "EXTENSION_LIMIT"
            }));
        }
        identity(&mut tx, caller, &scope).await?;
        let full_window: bool = sqlx::query_scalar(
            "SELECT cbgpu_schools.gpu_notebook_permission($1,$2,clock_timestamp(),$3)",
        )
        .bind(caller.user)
        .bind(school)
        .bind(change.ends_at)
        .fetch_one(&mut *tx)
        .await?;
        if !full_window {
            return Err(Error(StatusCode::FORBIDDEN, "GPU_PERMISSION_REQUIRED"));
        }
        // This one fixed-column trusted write bypasses the legacy owner-stop rewrite.
        // Caller claims/permissions were checked above, never inferred from this service context.
        sqlx::query("SELECT set_config('request.jwt.claims',$1,true)")
            .bind(
                json!({"sub":caller.user,"session_id":caller.session,"role":"service_role"})
                    .to_string(),
            )
            .execute(&mut *tx)
            .await?;
        let affected=match scope {
            Scope::Legacy=>sqlx::query("UPDATE public.reservations SET end_time=$1 WHERE id=$2 AND user_id=$3 AND end_time=$4 AND status='approved' AND start_time<=clock_timestamp() AND end_time>clock_timestamp()").bind(change.ends_at).bind(id).bind(caller.user).bind(booking.ends).execute(&mut *tx).await?.rows_affected(),
            Scope::School(_)=>sqlx::query("UPDATE cbgpu_schools.gpu_resource_reservations SET ends_at=$1 WHERE id=$2 AND owner_id=$3 AND ends_at=$4 AND status='RESERVED' AND starts_at<=clock_timestamp() AND ends_at>clock_timestamp()").bind(change.ends_at).bind(id).bind(caller.user).bind(booking.ends).execute(&mut *tx).await?.rows_affected(),
        };
        if affected != 1 {
            return Err(conflict("RESERVATION_EXPIRED"));
        }
        tx.commit().await?;
        let can_extend = max_end.is_none_or(|v| v > change.ends_at);
        let reason = if can_extend {
            None
        } else {
            Some(if next.is_some_and(|v| v <= change.ends_at) {
                "RESERVATION_CONFLICT"
            } else {
                "EXTENSION_LIMIT"
            })
        };
        return Ok(
            json!({"reservation_id":id,"current_end":change.ends_at,"max_end":max_end,"server_now":now,"can_extend":can_extend,"reason":reason,"updated":true}),
        );
    }
    tx.rollback().await?;
    Ok(
        json!({"reservation_id":id,"current_end":booking.ends,"max_end":max_end,"server_now":now,"can_extend":reason.is_none(),"reason":reason}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn claims_must_match_verified_auth_user_and_contain_a_session() {
        let uid = Uuid::new_v4();
        let sid = Uuid::new_v4();
        let token = format!(
            "Bearer e30.{}.signature",
            URL_SAFE_NO_PAD.encode(json!({"sub":uid,"session_id":sid,"exp":123}).to_string())
        );
        let claims = verified_claims(&token, uid).unwrap();
        assert_eq!(claims.session, sid);
        assert_eq!(claims.expires, 123);
        assert!(verified_claims(&token, Uuid::new_v4()).is_none());
        for value in [
            json!({"sub":uid,"exp":123}),
            json!({"sub":uid,"session_id":sid,"exp":"123"}),
        ] {
            assert!(
                verified_claims(
                    &format!(
                        "Bearer e30.{}.signature",
                        URL_SAFE_NO_PAD.encode(value.to_string())
                    ),
                    uid
                )
                .is_none()
            );
        }
    }
    #[test]
    fn exact_paths_methods_and_body_shape_only() {
        let id = Uuid::new_v4();
        assert!(
            route(
                &format!("/v1/gpu/legacy/reservations/{id}/extension"),
                &Method::GET
            )
            .is_some()
        );
        assert!(
            route(
                &format!("/v1/schools/school-b/gpu/reservations/{id}/extend"),
                &Method::POST
            )
            .is_some()
        );
        for path in [
            format!("/v1/gpu/legacy/reservations/{id}/extend/"),
            format!("/v1/schools/%2F/gpu/reservations/{id}/extend"),
        ] {
            assert!(route(&path, &Method::POST).is_none());
        }
        assert!(
            route(
                &format!("/v1/gpu/legacy/reservations/{id}/extension"),
                &Method::POST
            )
            .is_none()
        );
        for body in [
            r#"{"ends_at":"infinity","expected_ends_at":"2030-01-01T00:00:00Z"}"#,
            r#"{"ends_at":"2030-01-02T00:00:00Z","expected_ends_at":"2030-01-01T00:00:00Z","user_id":"victim"}"#,
            r#"{"ends_at":"2030-01-02T00:00:00Z"}"#,
        ] {
            assert!(serde_json::from_str::<Extension>(body).is_err());
        }
    }

    #[tokio::test]
    async fn http_rejects_invalid_bodies_and_unverified_identity_before_sql() {
        use axum::{Router, body::Body, http::Request};
        use tower::ServiceExt;
        let uid = Uuid::new_v4();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            axum::serve(
                listener,
                Router::new().fallback(move || async move { Json(json!({"id":uid})) }),
            )
            .await
            .unwrap();
        });
        let config = Config {
            reservations: Some(
                sqlx::postgres::PgPoolOptions::new()
                    .connect_lazy("postgres://synthetic@127.0.0.1:1/synthetic")
                    .unwrap(),
            ),
            enabled: true,
            diagnostics: None,
            notebooks: None,
            backend_url: format!("http://{address}"),
            anon_key: "synthetic".into(),
            service_key: "synthetic".into(),
            client: reqwest::Client::new(),
        };
        let app = crate::router(config);
        let uri = format!("/v1/gpu/legacy/reservations/{}/extend", Uuid::new_v4());
        for (body, auth, status) in [
            ("{}", Some("Bearer synthetic"), StatusCode::BAD_REQUEST),
            (
                r#"{"ends_at":"2030-01-02T00:00:00Z","expected_ends_at":"2030-01-01T00:00:00Z"}"#,
                None,
                StatusCode::UNAUTHORIZED,
            ),
            (
                r#"{"ends_at":"2030-01-02T00:00:00Z","expected_ends_at":"2030-01-01T00:00:00Z"}"#,
                Some("Bearer synthetic"),
                StatusCode::UNAUTHORIZED,
            ),
        ] {
            let mut request = Request::post(&uri);
            if let Some(auth) = auth {
                request = request.header("authorization", auth);
            }
            let response = app
                .clone()
                .oneshot(request.body(Body::from(body)).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), status);
        }
        task.abort();
    }
}
