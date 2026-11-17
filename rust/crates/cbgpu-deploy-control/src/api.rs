//! Private business API. The public tenant HTTP/WS ingress remains a separate boundary.
use crate::{Result, authority::Authority, config, fail, now, store::Store, valid_id};
use axum::{
    Json, Router,
    body::to_bytes,
    extract::{Request, State},
    http::{Method, StatusCode},
    response::{IntoResponse, Response},
};
use serde_json::{Value, json};
use sqlx::{Postgres, Row, Transaction};
use std::sync::Arc;
pub struct Api {
    pub workers_enabled: std::sync::atomic::AtomicBool,
    pub store: Store,
    pub authority: Arc<dyn Authority>,
}
pub fn router(api: Arc<Api>) -> Router {
    Router::new().fallback(handle).with_state(api)
}
async fn body(request: Request) -> Result<Value> {
    let bytes = to_bytes(request.into_body(), 65536)
        .await
        .map_err(|_| fail("Request too large", 413))?;
    let value: Value = serde_json::from_slice(if bytes.is_empty() { b"{}" } else { &bytes })?;
    if !value.is_object() {
        return Err(fail("Use a JSON object", 400));
    }
    Ok(value)
}
async fn handle(State(api): State<Arc<Api>>, request: Request) -> Response {
    let result = process(&api, request).await;
    let mut response = match result {
        Ok(value) => value,
        Err(error) => error.into_response(),
    };
    response
        .headers_mut()
        .insert("cache-control", "no-store".parse().unwrap());
    response
        .headers_mut()
        .insert("x-content-type-options", "nosniff".parse().unwrap());
    response
}
async fn process(api: &Api, request: Request) -> Result<Response> {
    let path = request.uri().path().to_owned();
    let method = request.method().clone();
    if path == "/health" && method == Method::GET {
        return Ok(Json(json!({"ok":true,"candidate":true,"implementation":"native-rust","workers_attached":api.workers_enabled.load(std::sync::atomic::Ordering::SeqCst)})).into_response());
    }
    let headers = request.headers();
    if headers.get_all("x-cbgpu-deploy-key").iter().count() != 1
        || !api.store.cipher.internal_key_matches(
            headers
                .get("x-cbgpu-deploy-key")
                .and_then(|v| v.to_str().ok())
                .unwrap_or(""),
        )
    {
        return Err(fail("Forbidden", 403));
    }
    if headers.get_all("x-cbgpu-owner").iter().count() != 1 {
        return Err(fail("Verified owner required", 403));
    }
    let actor = headers
        .get("x-cbgpu-owner")
        .and_then(|v| v.to_str().ok())
        .filter(|v| valid_id(v))
        .ok_or_else(|| fail("Verified owner required", 403))?
        .to_owned();
    api.authority.identity(&actor).await?;
    if path.contains('%')
        || path.contains("//")
        || path.ends_with('/')
        || request.uri().query().is_some()
    {
        return Err(fail("Not found", 404));
    }
    let parts: Vec<_> = path.trim_start_matches('/').split('/').collect();
    match parts.first().copied() {
        Some("projects") => {
            if parts.len() == 1 && method == Method::GET {
                return Ok(Json(api.store.list(&actor).await?).into_response());
            }
            if parts.len() == 1 && method == Method::POST {
                let input = body(request).await?;
                let draft = input["draft"] == true;
                if !draft && !api.authority.can_execute(&actor).await? {
                    return Err(fail(
                        "School compute permission required; save a draft instead",
                        403,
                    ));
                }
                let p = api.store.create(&actor, &input).await?;
                let deployment = if draft {
                    None
                } else {
                    Some(api.store.schedule(&p, &actor).await?)
                };
                return Ok((
                    StatusCode::CREATED,
                    Json(json!({"project":api.store.output(&p).await?,"deployment_id":deployment})),
                )
                    .into_response());
            }
            let p = api
                .store
                .find(parts.get(1).copied().unwrap_or(""), &actor)
                .await?;
            if parts.len() == 2 {
                if method == Method::GET {
                    return Ok(Json(json!({"project":api.store.output(&p).await?})).into_response());
                }
                if method == Method::PATCH {
                    let input = body(request).await?;
                    let updated = api.store.patch(&p, &input).await?;
                    return Ok(
                        Json(json!({"project":api.store.output(&updated).await?})).into_response()
                    );
                }
                if method == Method::DELETE {
                    api.store.delete(&p).await?;
                    return Ok(Json(json!({"ok":true})).into_response());
                }
            }
            if parts.len() == 3 && parts[2] == "deploy" && method == Method::POST {
                can_build(api, &actor, &p.owner).await?;
                return Ok((
                    StatusCode::ACCEPTED,
                    Json(json!({"deployment_id":api.store.schedule(&p,&actor).await?})),
                )
                    .into_response());
            }
            if parts.len() == 3 && parts[2] == "rollback" && method == Method::POST {
                can_build(api, &actor, &p.owner).await?;
                let input = body(request).await?;
                let updated = api
                    .store
                    .rollback(&p, input["deployment_id"].as_str().unwrap_or(""))
                    .await?;
                return Ok(
                    Json(json!({"project":api.store.output(&updated).await?})).into_response()
                );
            }
            if parts.len() == 4 && parts[2] == "logs" && method == Method::GET {
                return Ok(Json(api.store.logs(&p, parts[3]).await?).into_response());
            }
        }
        Some("teams") => {
            if parts.len() != 3 {
                return Err(fail("Not found", 404));
            }
            let kind = kind(parts[1])?;
            let id = resource_id(parts[2])?;
            let owner = resource_owner(api, kind, id, &actor).await?;
            let role = api.store.role(kind, id, &owner, &actor).await?;
            if method == Method::GET {
                let rows=sqlx::query("SELECT user_id,role,email,name,created_at FROM deploy_store.project_members WHERE resource_type=$1 AND project_id=$2 ORDER BY created_at").bind(kind).bind(id).fetch_all(&api.store.pool).await?;
                let members:Vec<_>=rows.into_iter().map(|r|json!({"user_id":r.get::<String,_>("user_id"),"role":r.get::<String,_>("role"),"email":r.get::<String,_>("email"),"name":r.get::<String,_>("name"),"created_at":r.get::<String,_>("created_at")})).collect();
                return Ok(
                    Json(json!({"owner_id":owner,"my_role":role,"members":members}))
                        .into_response(),
                );
            }
            let input = body(request).await?;
            let user = input["user_id"]
                .as_str()
                .filter(|v| valid_id(v) && *v != owner)
                .ok_or_else(|| fail("Invalid collaborator", 400))?;
            if method == Method::POST {
                if actor != owner {
                    return Err(fail("Only the owner can manage collaborators", 403));
                }
                let member_role = input["role"]
                    .as_str()
                    .filter(|v| ["editor", "viewer"].contains(v))
                    .ok_or_else(|| fail("Invalid collaborator role", 400))?;
                let email = input["email"]
                    .as_str()
                    .filter(|v| v.len() <= 320)
                    .ok_or_else(|| fail("Invalid collaborator email", 400))?;
                let name = input["name"]
                    .as_str()
                    .filter(|v| v.len() <= 200)
                    .ok_or_else(|| fail("Invalid collaborator name", 400))?;
                api.authority.identity(user).await?;
                let mut tx = api.store.pool.begin().await?;
                lock_resource(api, &mut tx, kind, id, &owner, &actor).await?;
                let current:i64=sqlx::query_scalar("SELECT count(*)FROM deploy_store.project_members WHERE resource_type=$1 AND project_id=$2").bind(kind).bind(id).fetch_one(&mut*tx).await?;
                let exists:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM deploy_store.project_members WHERE resource_type=$1 AND project_id=$2 AND user_id=$3)").bind(kind).bind(id).bind(user).fetch_one(&mut*tx).await?;
                if current >= 20 && !exists {
                    return Err(fail("Maximum20 collaborators", 429));
                }
                sqlx::query("INSERT INTO deploy_store.project_members(resource_type,project_id,user_id,role,email,name,created_at)VALUES($1,$2,$3,$4,$5,$6,$7)ON CONFLICT(resource_type,project_id,user_id)DO UPDATE SET role=excluded.role,email=excluded.email,name=excluded.name").bind(kind).bind(id).bind(user).bind(member_role).bind(email).bind(name).bind(now()).execute(&mut*tx).await?;
                tx.commit().await?;
                return Ok(Json(json!({"ok":true})).into_response());
            }
            if method == Method::DELETE {
                if actor != owner && user != actor {
                    return Err(fail("Only the owner can remove other collaborators", 403));
                }
                let mut tx = api.store.pool.begin().await?;
                lock_resource(api, &mut tx, kind, id, &owner, &actor).await?;
                sqlx::query("DELETE FROM deploy_store.project_members WHERE resource_type=$1 AND project_id=$2 AND user_id=$3").bind(kind).bind(id).bind(user).execute(&mut*tx).await?;
                tx.commit().await?;
                return Ok(Json(json!({"ok":true})).into_response());
            }
        }
        Some("domains") => {
            if !(3..=4).contains(&parts.len()) {
                return Err(fail("Not found", 404));
            }
            let kind = kind(parts[1])?;
            let id = resource_id(parts[2])?;
            let owner = resource_owner(api, kind, id, &actor).await?;
            api.store.role(kind, id, &owner, &actor).await?;
            if method == Method::GET && parts.len() == 3 {
                return Ok(Json(
                    json!({"can_manage":actor==owner,"domains":domains(api,kind,id).await?}),
                )
                .into_response());
            }
            if actor != owner {
                return Err(fail("Only the owner can manage domains", 403));
            }
            let input = body(request).await?;
            if method == Method::POST && parts.len() == 3 {
                let host = config::hostname(input["hostname"].as_str().unwrap_or(""))?;
                let mut tx = api.store.pool.begin().await?;
                lock_resource(api, &mut tx, kind, id, &owner, &actor).await?;
                let count:i64=sqlx::query_scalar("SELECT count(*)FROM deploy_store.custom_domains WHERE resource_type=$1 AND project_id=$2").bind(kind).bind(id).fetch_one(&mut*tx).await?;
                if count >= 5 {
                    return Err(fail("Maximum5 custom domains per project", 429));
                }
                let exists:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM deploy_store.custom_domains WHERE resource_type=$1 AND project_id=$2 AND hostname=$3)").bind(kind).bind(id).bind(&host).fetch_one(&mut*tx).await?;
                if exists {
                    return Err(fail("Domain already added", 409));
                }
                let domain_id = uuid::Uuid::new_v4().to_string();
                let token = format!(
                    "{}{}",
                    uuid::Uuid::new_v4().simple(),
                    &uuid::Uuid::new_v4().simple().to_string()[..16]
                );
                sqlx::query("INSERT INTO deploy_store.custom_domains(id,hostname,resource_type,project_id,owner,verification_token,created_at)VALUES($1,$2,$3,$4,$5,$6,$7)").bind(&domain_id).bind(host).bind(kind).bind(id).bind(&owner).bind(token).bind(now()).execute(&mut*tx).await?;
                tx.commit().await?;
                return Ok((
                    StatusCode::CREATED,
                    Json(json!({"domain":domain_by_id(api,&domain_id,kind,id).await?})),
                )
                    .into_response());
            }
            let domain_id = input["domain_id"]
                .as_str()
                .filter(|v| valid_id(v))
                .ok_or_else(|| fail("Domain not found", 404))?;
            let row=sqlx::query("SELECT * FROM deploy_store.custom_domains WHERE id=$1 AND resource_type=$2 AND project_id=$3").bind(domain_id).bind(kind).bind(id).fetch_optional(&api.store.pool).await?.ok_or_else(||fail("Domain not found",404))?;
            if method == Method::DELETE && parts.len() == 3 {
                let mut tx = api.store.pool.begin().await?;
                lock_resource(api, &mut tx, kind, id, &owner, &actor).await?;
                sqlx::query("DELETE FROM deploy_store.custom_domains WHERE id=$1 AND resource_type=$2 AND project_id=$3").bind(domain_id).bind(kind).bind(id).execute(&mut*tx).await?;
                if let Some(provider) = row.get::<Option<String>, _>("provider_id") {
                    Store::enqueue(
                        &mut tx,
                        "domain_remove",
                        kind,
                        id,
                        &owner,
                        None,
                        json!({"provider_id":provider,"hostname":row.get::<String,_>("hostname")}),
                    )
                    .await?;
                }
                tx.commit().await?;
                return Ok(Json(json!({"ok":true})).into_response());
            }
            if method == Method::POST && parts.get(3) == Some(&"verify") {
                let host: String = row.get("hostname");
                let token: String = row.get("verification_token");
                let verified = api
                    .authority
                    .txt_ownership(&host, &format!("cbgpu-domain-verification={token}"))
                    .await?;
                let mut tx = api.store.pool.begin().await?;
                lock_resource(api, &mut tx, kind, id, &owner, &actor).await?;
                sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended('deploy:domains',0))")
                    .execute(&mut *tx)
                    .await?;
                if !verified {
                    sqlx::query("UPDATE deploy_store.custom_domains SET dns_verified=0,tls_status='pending_dns',last_check=$1 WHERE id=$2").bind(now()).bind(domain_id).execute(&mut*tx).await?;
                } else {
                    let active:i64=sqlx::query_scalar("SELECT count(*)FROM deploy_store.custom_domains WHERE dns_verified=1 AND id<>$1").bind(domain_id).fetch_one(&mut*tx).await?;
                    if active >= 90 && row.get::<i32, _>("dns_verified") == 0 {
                        return Err(fail("Custom-domain capacity reached", 429));
                    }
                    sqlx::query("UPDATE deploy_store.custom_domains SET dns_verified=0,tls_status='ownership_replaced'WHERE hostname=$1 AND id<>$2").bind(&host).bind(domain_id).execute(&mut*tx).await?;
                    sqlx::query("UPDATE deploy_store.custom_domains SET dns_verified=1,tls_status='pending_tls',last_check=$1 WHERE id=$2").bind(now()).bind(domain_id).execute(&mut*tx).await?;
                    Store::enqueue(&mut tx,"domain_ensure",kind,id,&owner,None,json!({"domain_id":domain_id,"hostname":host,"provider_id":row.get::<Option<String>,_>("provider_id")})).await?;
                }
                tx.commit().await?;
                return Ok(
                    Json(json!({"domain":domain_by_id(api,domain_id,kind,id).await?}))
                        .into_response(),
                );
            }
        }
        Some("servers") => {
            if parts.len() != 1 {
                return Err(fail("Not found", 404));
            }
            if method == Method::GET {
                let rows = sqlx::query(
                    "SELECT project_id,slug FROM deploy_store.server_domains WHERE owner=$1",
                )
                .bind(&actor)
                .fetch_all(&api.store.pool)
                .await?;
                let domains:Vec<_>=rows.into_iter().map(|r|json!({"project_id":r.get::<String,_>("project_id"),"slug":r.get::<String,_>("slug")})).collect();
                let rows=sqlx::query("SELECT project_id,role FROM deploy_store.project_members WHERE resource_type='server'AND user_id=$1").bind(&actor).fetch_all(&api.store.pool).await?;
                let shared:Vec<_>=rows.into_iter().map(|r|json!({"project_id":r.get::<String,_>("project_id"),"role":r.get::<String,_>("role")})).collect();
                return Ok(
                    Json(json!({"domains":domains,"shared_project_ids":shared})).into_response()
                );
            }
            let input = body(request).await?;
            let id = resource_id(input["project_id"].as_str().unwrap_or(""))?;
            let owner = api.authority.server_owner(id).await?;
            if actor != owner {
                return Err(fail("Project not found", 404));
            }
            let mut tx = api.store.pool.begin().await?;
            lock_resource(api, &mut tx, "server", id, &owner, &actor).await?;
            if method == Method::DELETE {
                sqlx::query(
                    "DELETE FROM deploy_store.server_domains WHERE project_id=$1 AND owner=$2",
                )
                .bind(id)
                .bind(&owner)
                .execute(&mut *tx)
                .await?;
                tx.commit().await?;
                return Ok(Json(json!({"ok":true})).into_response());
            }
            if method == Method::POST {
                let slug = input["slug"]
                    .as_str()
                    .filter(|s| {
                        s.len() >= 3
                            && s.len() <= 35
                            && !s.ends_with('-')
                            && s.as_bytes()[0].is_ascii_lowercase()
                            && s.bytes()
                                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
                    })
                    .ok_or_else(|| fail("Invalid server name", 400))?;
                sqlx::query("INSERT INTO deploy_store.server_domains(project_id,owner,slug)VALUES($1,$2,$3)ON CONFLICT(project_id)DO UPDATE SET slug=excluded.slug WHERE deploy_store.server_domains.owner=excluded.owner").bind(id).bind(&owner).bind(slug).execute(&mut*tx).await?;
                tx.commit().await?;
                return Ok(Json(
                    json!({"url":format!("https://{slug}.server.cbgpu.com"),"slug":slug}),
                )
                .into_response());
            }
        }
        Some("compose") => {
            if parts.len() != 2 {
                return Err(fail("Not found", 404));
            }
            let id = resource_id(parts[1])?;
            let owner = api.authority.server_owner(id).await?;
            let role = api.store.role("server", id, &owner, &actor).await?;
            if method == Method::GET {
                return Ok(Json(compose_summary(api, id).await?).into_response());
            }
            if method == Method::DELETE && role == "owner" {
                let mut tx = api.store.pool.begin().await?;
                lock_resource(api, &mut tx, "server", id, &owner, &actor).await?;
                sqlx::query("UPDATE deploy_store.work_items SET cancel_requested=true WHERE kind='compose_build'AND project_id=$1 AND state IN('queued','running')").bind(id).execute(&mut*tx).await?;
                let operation = Store::enqueue(
                    &mut tx,
                    "compose_stop",
                    "server",
                    id,
                    &owner,
                    None,
                    json!({}),
                )
                .await?;
                tx.commit().await?;
                return Ok(Json(json!({"ok":true,"operation_id":operation})).into_response());
            }
        }
        _ => {}
    }
    Err(fail("Not found", 404))
}
async fn can_build(api: &Api, actor: &str, owner: &str) -> Result<()> {
    if !api.authority.can_execute(actor).await?
        || (actor != owner && !api.authority.can_execute(owner).await?)
    {
        return Err(fail(
            "School compute permission required; project retained",
            403,
        ));
    }
    Ok(())
}
fn kind(value: &str) -> Result<&str> {
    if ["frontend", "server"].contains(&value) {
        Ok(value)
    } else {
        Err(fail("Not found", 404))
    }
}
fn resource_id(value: &str) -> Result<&str> {
    if valid_id(value) {
        Ok(value)
    } else {
        Err(fail("Project not found", 404))
    }
}
async fn resource_owner(api: &Api, kind: &str, id: &str, actor: &str) -> Result<String> {
    if kind == "frontend" {
        Ok(api.store.find(id, actor).await?.owner)
    } else {
        api.authority.server_owner(id).await
    }
}
async fn lock_resource(
    api: &Api,
    tx: &mut Transaction<'_, Postgres>,
    kind: &str,
    id: &str,
    owner: &str,
    actor: &str,
) -> Result<()> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
        .bind(format!("deploy-resource:{kind}:{id}"))
        .execute(&mut **tx)
        .await?;
    if kind == "frontend" {
        Store::lock_role(tx, id, actor).await?;
    } else {
        let fresh = api.authority.server_owner(id).await?;
        if fresh != owner {
            return Err(fail("Project ownership changed", 409));
        }
        if owner != actor {
            let exists:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM deploy_store.project_members WHERE resource_type='server'AND project_id=$1 AND user_id=$2)").bind(id).bind(actor).fetch_one(&mut**tx).await?;
            if !exists {
                return Err(fail("Project not found", 404));
            }
        }
    }
    Ok(())
}
async fn target(api: &Api, kind: &str, id: &str) -> Result<String> {
    if kind == "frontend" {
        let slug: String = sqlx::query_scalar("SELECT slug FROM deploy_store.projects WHERE id=$1")
            .bind(id)
            .fetch_one(&api.store.pool)
            .await?;
        Ok(format!("{slug}.front.cbgpu.com"))
    } else {
        let slug: Option<String> =
            sqlx::query_scalar("SELECT slug FROM deploy_store.server_domains WHERE project_id=$1")
                .bind(id)
                .fetch_optional(&api.store.pool)
                .await?;
        Ok(format!(
            "{}.server.cbgpu.com",
            slug.unwrap_or_else(|| format!("server-{}", &id[..8]))
        ))
    }
}
async fn domain_output(api: &Api, row: sqlx::postgres::PgRow) -> Result<Value> {
    let hostname: String = row.get("hostname");
    let kind: String = row.get("resource_type");
    let project: String = row.get("project_id");
    let token: String = row.get("verification_token");
    let mut records = vec![
        json!({"type":"CNAME","name":hostname,"value":target(api,&kind,&project).await?}),
        json!({"type":"TXT","name":format!("_cbgpu-verification.{hostname}"),"value":format!("cbgpu-domain-verification={token}")}),
    ];
    records.extend(
        row.get::<Value, _>("provider_records")
            .as_array()
            .cloned()
            .unwrap_or_default(),
    );
    Ok(
        json!({"id":row.get::<String,_>("id"),"hostname":hostname,"dns_verified":row.get::<i32,_>("dns_verified")!=0,"tls_status":row.get::<Option<String>,_>("tls_status"),"last_check":row.get::<Option<String>,_>("last_check"),"records":records}),
    )
}
async fn domains(api: &Api, kind: &str, id: &str) -> Result<Vec<Value>> {
    let rows=sqlx::query("SELECT * FROM deploy_store.custom_domains WHERE resource_type=$1 AND project_id=$2 ORDER BY created_at").bind(kind).bind(id).fetch_all(&api.store.pool).await?;
    let mut result = vec![];
    for row in rows {
        result.push(domain_output(api, row).await?)
    }
    Ok(result)
}
async fn domain_by_id(api: &Api, id: &str, kind: &str, project: &str) -> Result<Value> {
    let row=sqlx::query("SELECT * FROM deploy_store.custom_domains WHERE id=$1 AND resource_type=$2 AND project_id=$3").bind(id).bind(kind).bind(project).fetch_optional(&api.store.pool).await?.ok_or_else(||fail("Domain not found",404))?;
    domain_output(api, row).await
}
pub async fn compose_summary(api: &Api, id: &str) -> Result<Value> {
    let row = sqlx::query("SELECT * FROM deploy_store.compose_states WHERE project_id=$1")
        .bind(id)
        .fetch_optional(&api.store.pool)
        .await?;
    let Some(row) = row else {
        return Ok(
            json!({"status":"queued","logs":"Waiting for Compose deployment…","persistent_storage_gb":40}),
        );
    };
    let result = row
        .get::<Option<Value>, _>("attempt_result")
        .or_else(|| row.get("result"))
        .unwrap_or(Value::Null);
    let diagnostics: Option<Value> = row.get("diagnostics");
    Ok(
        json!({"failed_services":diagnostics.as_ref().map(|v|v["failed_services"].clone()).unwrap_or(json!([])),"diagnostics":diagnostics,"status":row.get::<String,_>("status"),"logs":row.get::<Option<String>,_>("logs"),"error":row.get::<Option<String>,_>("error"),"updated_at":row.get::<String,_>("updated_at"),"services":result["services"].as_array().cloned().unwrap_or_default(),"gateway_service":result["gateway"],"persistent_storage_gb":row.get::<Option<i64>,_>("storage_gb"),"storage_used_bytes":row.get::<Option<i64>,_>("storage_used_bytes"),"volumes_preserved":true}),
    )
}
