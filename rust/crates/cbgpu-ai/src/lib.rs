//! Private native Axum AI control plane. No Python proxy or fallback.
use axum::{
    Router,
    body::{Body, to_bytes},
    extract::{Request, State},
    http::{HeaderMap, Method, StatusCode},
    response::{IntoResponse, Response},
    routing::any,
};
use cbgpu_ai_store::{Failure, Result, Store, conversations::SendRequest, fail, now, valid_id};
use futures_util::StreamExt;
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::{Arc, Mutex},
};
use subtle::ConstantTimeEq;
pub mod preparation;
pub mod resources;
pub mod runner;
pub mod scheduler;
pub mod web;
/// Server-side scope flags are authoritative; malformed configuration must not
/// quietly select the old primary-profile authorization path.
pub fn scoped_authorization_mode(values: impl IntoIterator<Item = Option<String>>) -> Result<bool> {
    let mut enabled = false;
    for value in values.into_iter().flatten() {
        match value.trim().to_ascii_lowercase().as_str() {
            "" | "0" | "false" => {}
            "1" | "true" => enabled = true,
            _ => return Err(fail("Invalid school authorization configuration", 503)),
        }
    }
    Ok(enabled)
}
pub fn scoped_authorization_from_environment() -> Result<bool> {
    let values = [
        "CBGPU_MULTISCHOOL_V1",
        "CBGPU_ACCOUNT_V2",
        "NEXT_PUBLIC_CBGPU_ACCOUNT_V2",
    ]
    .into_iter()
    .map(|key| match std::env::var(key) {
        Ok(value) => Ok(Some(value)),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(_) => Err(fail("Invalid school authorization configuration", 503)),
    })
    .collect::<Result<Vec<_>>>()?;
    scoped_authorization_mode(values)
}
#[derive(Clone)]
pub struct Authorization {
    pub backend_url: String,
    pub service_key: String,
    pub account_v2: bool,
    pub client: reqwest::Client,
}
impl Authorization {
    /// Identity remains independent of school GPU rights: graduation/pending
    /// affiliation retains owned records, suspension/deletion does not.
    pub async fn identity(&self, owner: &str) -> Result<()> {
        if !valid_id(owner) {
            return Err(fail("Authentication required", 401));
        }
        let response = self
            .client
            .get(format!("{}/auth/v1/admin/users/{owner}", self.backend_url))
            .header("apikey", &self.service_key)
            .bearer_auth(&self.service_key)
            .send()
            .await
            .map_err(|_| fail("Identity verification unavailable", 503))?;
        if !response.status().is_success() {
            return Err(fail("Identity verification unavailable", 503));
        }
        let raw: Value = response
            .json()
            .await
            .map_err(|_| fail("Identity verification unavailable", 503))?;
        let account = raw.get("user").filter(|v| v.is_object()).unwrap_or(&raw);
        if account["id"] != owner || account.get("deleted_at").is_some_and(|v| !v.is_null()) {
            return Err(fail("Account unavailable", 403));
        }
        if let Some(value) = account["banned_until"].as_str().filter(|v| !v.is_empty()) {
            let until = chrono::DateTime::parse_from_rfc3339(value)
                .map_err(|_| fail("Identity verification unavailable", 503))?;
            if until.timestamp() as f64 > now() {
                return Err(fail("Account unavailable", 403));
            }
        }
        let columns = if self.account_v2 {
            "id,account_status"
        } else {
            "id"
        };
        let response = self
            .client
            .get(format!("{}/rest/v1/profiles", self.backend_url))
            .query(&[("id", format!("eq.{owner}")), ("select", columns.into())])
            .header("apikey", &self.service_key)
            .bearer_auth(&self.service_key)
            .send()
            .await
            .map_err(|_| fail("Identity verification unavailable", 503))?;
        if !response.status().is_success() {
            return Err(fail("Identity verification unavailable", 503));
        }
        let rows: Value = response
            .json()
            .await
            .map_err(|_| fail("Identity verification unavailable", 503))?;
        let profile = rows
            .as_array()
            .filter(|v| v.len() == 1)
            .and_then(|v| v.first())
            .filter(|v| v["id"] == owner)
            .ok_or_else(|| fail("Account unavailable", 403))?;
        if self.account_v2
            && !matches!(
                profile["account_status"].as_str(),
                Some("ACTIVE" | "PENDING_VERIFICATION" | "REVERIFICATION_REQUIRED")
            )
        {
            return Err(fail("Account unavailable", 403));
        }
        Ok(())
    }
    pub async fn execution(&self, owner: &str) -> Result<()> {
        if !valid_id(owner) {
            return Err(fail("Authentication required", 401));
        }
        if !self.account_v2 {
            let response = self
                .client
                .get(format!("{}/auth/v1/admin/users/{owner}", self.backend_url))
                .header("apikey", &self.service_key)
                .bearer_auth(&self.service_key)
                .send()
                .await
                .map_err(|_| fail("School identity verification unavailable", 503))?;
            if !response.status().is_success() {
                return Err(fail("School identity verification unavailable", 503));
            }
            let response: Value = response
                .json()
                .await
                .map_err(|_| fail("School identity verification unavailable", 503))?;
            let user = response
                .get("user")
                .filter(|v| v.is_object())
                .unwrap_or(&response);
            if user["id"] != owner || user.get("deleted_at").is_some_and(|v| !v.is_null()) {
                return Err(fail("Account is unavailable", 403));
            }
            if let Some(banned) = user["banned_until"].as_str().filter(|s| !s.is_empty()) {
                let until = chrono::DateTime::parse_from_rfc3339(banned)
                    .map_err(|_| fail("School identity verification unavailable", 503))?;
                if until.timestamp() as f64 > now() {
                    return Err(fail("Account is suspended", 403));
                }
            }
        }
        let mut req = if self.account_v2 {
            self.client
                .post(format!(
                    "{}/rest/v1/rpc/cbgpu_service_can_use_resource",
                    self.backend_url
                ))
                .json(&json!({"user_id":owner,"resource":"gpu","target_school":"cbsh"}))
        } else {
            self.client
                .get(format!("{}/rest/v1/profiles", self.backend_url))
                .query(&[
                    ("id", format!("eq.{owner}")),
                    ("select", "id,is_approved".into()),
                ])
        };
        req = req
            .header("apikey", &self.service_key)
            .bearer_auth(&self.service_key);
        let response = req
            .send()
            .await
            .map_err(|_| fail("School permission verification unavailable", 503))?;
        if !response.status().is_success() {
            return Err(fail("School permission verification unavailable", 503));
        }
        let value: Value = response
            .json()
            .await
            .map_err(|_| fail("School permission verification unavailable", 503))?;
        let allowed = if self.account_v2 {
            value == true
        } else {
            value.as_array().is_some_and(|rows| {
                rows.len() == 1 && rows[0]["id"] == owner && rows[0]["is_approved"] != false
            })
        };
        if !allowed {
            return Err(fail("Verified school GPU permission is required", 403));
        }
        Ok(())
    }
}
pub struct Api {
    pub store: Store,
    pub internal_key: Vec<u8>,
    pub authorization: Authorization,
    pub remote: cbgpu_robotics::remote::RemoteRobots,
    pub robotics: cbgpu_robotics::planning::Robotics,
    pub preparation: preparation::Preparation,
    pub web: web::WebSearch,
    pub scheduler: tokio::sync::RwLock<Value>,
    pub running: tokio::sync::RwLock<HashSet<String>>,
    rates: Mutex<HashMap<String, VecDeque<f64>>>,
    upload_slots: Arc<tokio::sync::Semaphore>,
}
struct Jobs {
    store: Store,
    authorization: Authorization,
}
#[async_trait::async_trait]
impl cbgpu_robotics::planning::JobBackend for Jobs {
    async fn enqueue(
        &self,
        owner: &str,
        kind: &str,
        data: Value,
        title: &str,
    ) -> cbgpu_robotics::Result<Value> {
        self.authorization
            .execution(owner)
            .await
            .map_err(robot_error)?;
        self.store
            .enqueue(owner, kind, data, title)
            .await
            .map_err(robot_error)
    }
    async fn job(&self, owner: &str, id: &str) -> cbgpu_robotics::Result<Value> {
        self.store.job(owner, id, true).await.map_err(robot_error)
    }
    async fn cancel(&self, owner: &str, id: &str) -> cbgpu_robotics::Result<Value> {
        self.store
            .cancel_job(owner, id)
            .await
            .map_err(robot_error)?;
        Ok(json!({"ok":true}))
    }
}
fn robot_error(e: Failure) -> cbgpu_robotics::Failure {
    cbgpu_robotics::Failure {
        message: e.message,
        status: e.status,
    }
}
fn from_robot(e: cbgpu_robotics::Failure) -> Failure {
    fail(&e.message, e.status)
}
impl Api {
    pub fn new(
        store: Store,
        key: Vec<u8>,
        authorization: Authorization,
        ocr_image: String,
    ) -> Self {
        let remote = cbgpu_robotics::remote::RemoteRobots::new(store.pool.clone());
        let robotics = cbgpu_robotics::planning::Robotics::new(
            store.pool.clone(),
            Arc::new(Jobs {
                store: store.clone(),
                authorization: authorization.clone(),
            }),
        );
        let preparation = preparation::Preparation::new(store.clone(), ocr_image);
        Self {
            store,
            internal_key: key,
            authorization,
            remote,
            robotics,
            preparation,
            web: web::WebSearch::default(),
            scheduler: tokio::sync::RwLock::new(
                json!({"available":false,"reason":"Native inference runner is not attached"}),
            ),
            running: tokio::sync::RwLock::new(HashSet::new()),
            rates: Mutex::new(HashMap::new()),
            upload_slots: Arc::new(tokio::sync::Semaphore::new(2)),
        }
    }
    fn authenticate(&self, h: &HeaderMap) -> Result<()> {
        let supplied = h.get("x-cbgpu-ai-key").map(|v| v.as_bytes()).unwrap_or(&[]);
        if self.internal_key.len() < 16 || !bool::from(supplied.ct_eq(&self.internal_key)) {
            return Err(fail("Internal authentication required", 401));
        }
        Ok(())
    }
    fn rate(&self, owner: &str) -> Result<()> {
        let at = now();
        let mut rates = self
            .rates
            .lock()
            .map_err(|_| fail("Rate service unavailable", 503))?;
        if rates.len() > 5000 {
            rates.retain(|_, r| r.back().is_some_and(|t| at - t < 60.));
        }
        let times = rates.entry(owner.into()).or_default();
        while times.front().is_some_and(|t| at - t > 60.) {
            times.pop_front();
        }
        if times.len() >= 40 {
            return Err(fail("Too many AI requests; try again shortly", 429));
        }
        times.push_back(at);
        Ok(())
    }
}
pub fn router(api: Arc<Api>) -> Router {
    Router::new().fallback(any(dispatch)).with_state(api)
}
fn json_response(status: u16, value: Value) -> Response {
    let mut response = (
        StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
        axum::Json(value),
    )
        .into_response();
    let h = response.headers_mut();
    h.insert("cache-control", "no-store".parse().unwrap());
    h.insert("x-content-type-options", "nosniff".parse().unwrap());
    response
}
async fn dispatch(State(api): State<Arc<Api>>, request: Request) -> Response {
    match handle(api, request).await {
        Ok(r) => r,
        Err(e) => json_response(e.status, json!({"message":e.message})),
    }
}
async fn handle(api: Arc<Api>, request: Request) -> Result<Response> {
    let method = request.method().clone();
    let uri = request.uri().clone();
    let segments = uri
        .path()
        .split('/')
        .filter(|p| !p.is_empty())
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let p = segments.iter().map(String::as_str).collect::<Vec<_>>();
    if p == ["health"] && method == Method::GET {
        return Ok(json_response(
            200,
            json!({"status":"ok","commit":std::env::var("CBGPU_RELEASE_SHA").unwrap_or("candidate".into()),"scheduler":api.scheduler.read().await.clone()}),
        ));
    }
    if p == ["health", "ready"] && method == Method::GET {
        let scheduler = api.scheduler.read().await.clone();
        return Ok(json_response(
            if scheduler["available"] == true {
                200
            } else {
                503
            },
            json!({"scheduler":scheduler}),
        ));
    }
    api.authenticate(request.headers())?;
    let owner = request
        .headers()
        .get("x-cbgpu-owner")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_owned();
    let special = p == ["authenticate-key"]
        || p == ["robotics", "agent", "enroll"]
        || p == ["robotics", "agent", "sync"];
    if !special && !valid_id(&owner) {
        return Err(fail("Authentication required", 401));
    }
    if !special && method != Method::GET {
        api.rate(&owner)?;
    }
    if p == ["uploads"] && method == Method::POST {
        api.authorization.identity(&owner).await?;
        let _slot = api
            .upload_slots
            .clone()
            .try_acquire_owned()
            .map_err(|_| fail("Uploads are busy; try again shortly", 429))?;
        let name = url::form_urlencoded::parse(uri.query().unwrap_or("").as_bytes())
            .find(|(k, _)| k == "name")
            .map(|(_, v)| v.to_string())
            .unwrap_or("upload".into());
        let path = api
            .store
            .root
            .join(format!(".incoming-{}", uuid::Uuid::new_v4()));
        let upload = preparation::Spool::new(path)?;
        let mut file = tokio::fs::OpenOptions::new()
            .write(true)
            .open(&upload.path)
            .await?;
        let mut stream = request.into_body().into_data_stream();
        let mut size = 0u64;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|_| fail("Upload was interrupted", 400))?;
            size += chunk.len() as u64;
            if size > cbgpu_ai_store::MAX_UPLOAD {
                return Err(fail("Upload limit is 200 MB", 413));
            }
            tokio::io::AsyncWriteExt::write_all(&mut file, &chunk).await?;
        }
        file.sync_all().await?;
        drop(file);
        let value = api
            .store
            .upload(&owner, &name, std::fs::File::open(&upload.path)?)
            .await?;
        return Ok(json_response(201, value));
    }
    let body = if method == Method::POST {
        let bytes = to_bytes(request.into_body(), 3 * 1024 * 1024)
            .await
            .map_err(|_| fail("Request body is too large", 413))?;
        let value: Value = serde_json::from_slice(&bytes).map_err(|_| fail("Invalid JSON", 400))?;
        if !value.is_object() {
            return Err(fail("Invalid task", 400));
        }
        value
    } else {
        json!({})
    };
    if !special && p != ["models"] {
        api.authorization.identity(&owner).await?;
    }
    let text = |key: &str| body[key].as_str().unwrap_or("");
    if p == ["authenticate-key"] && method == Method::POST {
        return Ok(json_response(200, {
            let owner = api.store.key_owner(text("key")).await?;
            api.authorization.identity(&owner).await?;
            json!({"owner":owner})
        }));
    }
    if p == ["robotics", "agent", "enroll"] && method == Method::POST {
        return Ok(json_response(
            200,
            api.remote.enroll(&body).await.map_err(from_robot)?,
        ));
    }
    if p == ["robotics", "agent", "sync"] && method == Method::POST {
        return Ok(json_response(
            200,
            api.remote.sync(&body).await.map_err(from_robot)?,
        ));
    }
    if p == ["web", "search"] && method == Method::POST {
        return Ok(json_response(
            200,
            api.web.search(&owner, text("query"), false).await?,
        ));
    }
    if p == ["web", "read"] && method == Method::POST {
        let (html, url) = web::fetch_public(text("url"), 1024 * 1024).await?;
        return Ok(json_response(
            200,
            json!({"url":url,"text":web::plain_text(&html).chars().take(6000).collect::<String>()}),
        ));
    }
    if p == ["models"] && method == Method::GET {
        return Ok(json_response(
            200,
            json!({"object":"list","data":models(&api.store.root)?}),
        ));
    }
    if p == ["status"] && method == Method::GET {
        let rows:Vec<(String,i64)>=sqlx::query_as("SELECT state,count(*) FROM ai_store.jobs WHERE state IN('queued','running','paused') GROUP BY state").fetch_all(&api.store.pool).await?;
        let queue = rows
            .into_iter()
            .map(|(k, v)| (k, json!(v)))
            .collect::<serde_json::Map<_, _>>();
        return Ok(json_response(
            200,
            json!({"scheduler":api.scheduler.read().await.clone(),"queue":queue,"corpus":{},"models":[{"id":"cbgpu-chat","task":"chat"},{"id":"cbgpu-embeddings","task":"embeddings"},{"id":"cbgpu-whisper","task":"transcription"}]}),
        ));
    }
    if p == ["projects"] {
        if method == Method::GET {
            return Ok(json_response(
                200,
                json!({"projects":api.store.projects(&owner).await?}),
            ));
        }
        if method == Method::POST {
            return Ok(json_response(
                201,
                json!({"project":api.store.create_project(&owner,text("title")).await?}),
            ));
        }
    }
    if p.len() == 2 && p[0] == "projects" && method == Method::DELETE {
        api.store.delete_project(&owner, p[1]).await?;
        return Ok(json_response(200, json!({"ok":true})));
    }
    if p.len() == 4 && p[0] == "projects" && p[2] == "files" && method == Method::DELETE {
        api.store.remove_project_file(&owner, p[1], p[3]).await?;
        return Ok(json_response(200, json!({"ok":true})));
    }
    if p == ["chats"] {
        if method == Method::GET {
            return Ok(json_response(
                200,
                json!({"chats":api.store.conversations(&owner).await?}),
            ));
        }
        if method == Method::POST {
            return Ok(json_response(
                201,
                json!({"chat":api.store.create_conversation(&owner,body["project"].as_str(),body["site_search"]==true).await?}),
            ));
        }
    }
    if p.len() >= 2 && p[0] == "chats" && valid_id(p[1]) {
        let chat = p[1];
        if p.len() == 2 {
            if method == Method::GET {
                return Ok(json_response(
                    200,
                    json!({"chat":api.store.conversation(&owner,chat).await?}),
                ));
            }
            if method == Method::POST {
                return Ok(json_response(
                    200,
                    json!({"chat":api.store.rename_conversation(&owner,chat,text("title")).await?}),
                ));
            }
            if method == Method::DELETE {
                api.store.delete_conversation(&owner, chat).await?;
                return Ok(json_response(200, json!({"ok":true})));
            }
        }
        if p == ["chats", chat, "live"] && method == Method::GET {
            return Ok(json_response(
                200,
                api.store.live_conversation(&owner, chat).await?,
            ));
        }
        if p == ["chats", chat, "messages"] && method == Method::POST {
            api.authorization.execution(&owner).await?;
            let mut request = SendRequest::text(text("content"));
            request.model = body["model"].as_str().unwrap_or("cbgpu-chat").into();
            request.effort = body["effort"].as_str().unwrap_or("medium").into();
            request.research = body["research"] == true;
            request.site_context = body.get("site_context").cloned();
            if let Some(a) = body.get("attachments") {
                let a = a
                    .as_array()
                    .ok_or_else(|| fail("Invalid attachments", 400))?;
                request.attachments = a
                    .iter()
                    .map(|v| {
                        v.as_str()
                            .map(str::to_owned)
                            .ok_or_else(|| fail("Invalid attachments", 400))
                    })
                    .collect::<Result<Vec<_>>>()?;
            }
            let own = api.store.conversation_owned(&owner, chat).await?;
            let mut files = if let Some(project) = own["project"].as_str() {
                api.store
                    .project_files(&owner, project)
                    .await?
                    .into_iter()
                    .filter_map(|v| v["id"].as_str().map(str::to_owned))
                    .collect::<Vec<_>>()
            } else {
                vec![]
            };
            for id in &request.attachments {
                if !files.contains(id) {
                    files.push(id.clone());
                }
            }
            if files.len() > 3 {
                return Err(fail("파일은 최대 3개까지 첨부할 수 있어요.", 400));
            }
            for id in files {
                request.file_sources.extend(
                    api.preparation
                        .evidence(&owner, &id, &request.content)
                        .await?,
                );
            }
            if request.research || body["web_search"] == true {
                let sources = api
                    .web
                    .search(&owner, &request.content, request.research)
                    .await?;
                request.web_sources = sources["sources"].as_array().cloned().unwrap_or_default();
            }
            return Ok(json_response(
                202,
                json!({"chat":api.store.send_message(&owner,chat,request).await?}),
            ));
        }
    }
    if p == ["keys"] {
        if method == Method::GET {
            return Ok(json_response(
                200,
                json!({"keys":api.store.keys(&owner).await?}),
            ));
        }
        if method == Method::POST {
            return Ok(json_response(
                201,
                api.store.create_key(&owner, text("name")).await?,
            ));
        }
    }
    if p.len() == 2 && p[0] == "keys" && method == Method::DELETE {
        api.store.revoke_key(&owner, p[1]).await?;
        return Ok(json_response(200, json!({"ok":true})));
    }
    if p == ["jobs"] {
        if method == Method::GET {
            return Ok(json_response(
                200,
                json!({"jobs":api.store.jobs(&owner).await?}),
            ));
        }
        if method == Method::POST {
            api.authorization.execution(&owner).await?;
            return Ok(json_response(
                202,
                json!({"job":api.store.enqueue(&owner,text("kind"),body["input"].clone(),text("title")).await?}),
            ));
        }
    }
    if p.len() >= 2 && p[0] == "jobs" && valid_id(p[1]) {
        let id = p[1];
        if p.len() == 2 {
            if method == Method::GET {
                return Ok(json_response(
                    200,
                    json!({"job":api.store.job(&owner,id,true).await?}),
                ));
            }
            if method == Method::DELETE {
                api.store.delete_job(&owner, id).await?;
                return Ok(json_response(200, json!({"ok":true})));
            }
        }
        if p == ["jobs", id, "cancel"] && method == Method::POST {
            api.store.cancel_job(&owner, id).await?;
            return Ok(json_response(200, json!({"ok":true})));
        }
        if p == ["jobs", id, "retry"] && method == Method::POST {
            api.authorization.execution(&owner).await?;
            if api.running.read().await.contains(id) {
                return Err(fail("The task is still stopping. Try again shortly.", 409));
            }
            let saved: Value =
                sqlx::query_scalar("SELECT input FROM ai_store.jobs WHERE id=$1 AND owner=$2")
                    .bind(id)
                    .bind(&owner)
                    .fetch_optional(&api.store.pool)
                    .await?
                    .ok_or_else(|| fail("Task not found", 404))?;
            let job = api.store.job(&owner, id, false).await?;
            if let Some(upload) = saved["upload_id"].as_str() {
                let prepared = job["kind"] == "python"
                    && api.store.workspace_root(id)?.join(".prepared").exists();
                if !prepared {
                    api.store.get_upload(&owner, upload).await?;
                }
            }
            return Ok(json_response(
                200,
                json!({"job":api.store.retry_job(&owner,id).await?}),
            ));
        }
        if p == ["jobs", id, "download"] && method == Method::POST {
            return Ok(json_response(
                201,
                api.store
                    .download_ticket(&owner, id, text("kind"), text("path"))
                    .await?,
            ));
        }
        if p == ["jobs", id, "artifact"] && method == Method::GET {
            let relative = url::form_urlencoded::parse(uri.query().unwrap_or("").as_bytes())
                .find(|(k, _)| k == "path")
                .map(|(_, v)| v.to_string())
                .unwrap_or_default();
            return file_response(api.store.artifact(&owner, id, &relative).await?);
        }
    }
    if p.len() == 2 && p[0] == "downloads" && method == Method::GET {
        return match api.store.download(&owner, p[1]).await? {
            cbgpu_ai_store::Download::Artifact(f) => file_response(f),
            cbgpu_ai_store::Download::Bytes(bytes) => Ok((
                [
                    ("content-type", "application/octet-stream"),
                    ("cache-control", "no-store"),
                ],
                bytes,
            )
                .into_response()),
        };
    }
    if p.len() >= 2 && p[0] == "uploads" && valid_id(p[1]) {
        let id = p[1];
        if p.len() == 2 {
            if method == Method::GET {
                return Ok(json_response(
                    200,
                    json!({"file":api.preparation.status(&owner,id).await?}),
                ));
            }
            if method == Method::DELETE {
                api.store.delete_upload(&owner, id).await?;
                return Ok(json_response(200, json!({"ok":true})));
            }
        }
        if p == ["uploads", id, "prepare"] && method == Method::POST {
            return Ok(json_response(
                202,
                json!({"file":api.preparation.start(&owner,id).await?}),
            ));
        }
        if p == ["uploads", id, "text"] && method == Method::GET {
            return Ok(json_response(
                200,
                json!({"text":api.preparation.text(&owner,id).await?}),
            ));
        }
    }
    if p == ["robotics", "devices"] {
        if method == Method::GET {
            return Ok(json_response(
                200,
                json!({"devices":api.remote.list(&owner).await.map_err(from_robot)?}),
            ));
        }
        if method == Method::POST {
            return Ok(json_response(
                201,
                api.remote
                    .create(&owner, text("name"))
                    .await
                    .map_err(from_robot)?,
            ));
        }
    }
    if p.len() >= 3 && p[..2] == ["robotics", "devices"] {
        let id = p[2];
        if p.len() == 3 {
            if method == Method::GET {
                return Ok(json_response(
                    200,
                    api.remote.state(&owner, id).await.map_err(from_robot)?,
                ));
            }
            if method == Method::DELETE {
                return Ok(json_response(
                    200,
                    api.remote.revoke(&owner, id).await.map_err(from_robot)?,
                ));
            }
        }
        if p.len() == 4 && method == Method::POST {
            let value = match p[3] {
                "control" => api.remote.control(&owner, id, &body).await,
                "goal" => api.remote.goal(&owner, id, &body).await,
                "stop" => api.remote.stop(&owner, id).await,
                "pair" => api.remote.re_pair(&owner, id).await,
                _ => return Err(fail("Not found", 404)),
            }
            .map_err(from_robot)?;
            return Ok(json_response(if p[3] == "goal" { 202 } else { 200 }, value));
        }
    }
    if p == ["robotics", "plans"] && method == Method::POST {
        return Ok(json_response(
            202,
            api.robotics
                .create(&owner, &body)
                .await
                .map_err(from_robot)?,
        ));
    }
    if p.len() == 3 && p[..2] == ["robotics", "plans"] {
        if method == Method::GET {
            return Ok(json_response(
                200,
                api.robotics.get(&owner, p[2]).await.map_err(from_robot)?,
            ));
        }
        if method == Method::DELETE {
            return Ok(json_response(
                200,
                api.robotics
                    .cancel(&owner, p[2])
                    .await
                    .map_err(from_robot)?,
            ));
        }
    }
    Err(fail("Not found", 404))
}
fn file_response(file: std::fs::File) -> Result<Response> {
    let len = file.metadata()?.len();
    let body = Body::from_stream(tokio_util::io::ReaderStream::new(
        tokio::fs::File::from_std(file),
    ));
    let mut response = Response::new(body);
    response.headers_mut().insert(
        "content-length",
        len.to_string()
            .parse()
            .map_err(|_| fail("Invalid file length", 500))?,
    );
    response
        .headers_mut()
        .insert("content-type", "application/octet-stream".parse().unwrap());
    response
        .headers_mut()
        .insert("cache-control", "no-store".parse().unwrap());
    Ok(response)
}
fn models(root: &std::path::Path) -> Result<Vec<Value>> {
    let catalog: Value = serde_json::from_str(include_str!("../../../../ai/models.json"))?;
    let mut result = vec![];
    for (id, key, name, description) in [
        (
            "cbgpu-oss",
            "oss",
            "GPT-OSS 20B",
            "OpenAI gpt-oss 20B · 추론과 분석",
        ),
        ("cbgpu-chat", "chat", "CBGPU Chat", "Qwen2.5 7B · 빠른 대화"),
        (
            "cbgpu-coder",
            "coder",
            "CBGPU Coder",
            "Qwen2.5 Coder 7B · 코드와 에이전트",
        ),
        (
            "cbgpu-pro",
            "pro",
            "CBGPU Pro",
            "Qwen2.5 14B · 글쓰기와 분석",
        ),
    ] {
        let ready = (id != "cbgpu-oss" || root.join("llama-next").is_dir())
            && catalog[key]["files"].as_array().is_some_and(|a| {
                a.iter().all(|f| {
                    f.as_str()
                        .is_some_and(|f| root.join("models").join(key).join(f).is_file())
                })
            });
        result.push(json!({"id":id,"object":"model","owned_by":"cbgpu","name":name,"description":description,"ready":ready,"context_window":8192,"max_output_tokens":if id=="cbgpu-oss"{4096}else{2048},"capabilities":["text","tools"]}));
    }
    for (id, name, file, cap) in [
        (
            "cbgpu-embeddings",
            "Multilingual E5",
            "embed/model.safetensors",
            "embeddings",
        ),
        (
            "cbgpu-whisper",
            "Whisper large-v3",
            "transcribe/model.bin",
            "transcription",
        ),
    ] {
        result.push(json!({"id":id,"object":"model","owned_by":"cbgpu","name":name,"ready":root.join("models").join(file).is_file(),"capabilities":[cap]}));
    }
    Ok(result)
}
