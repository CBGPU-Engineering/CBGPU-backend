//! Tenant serving is separate from the private business API: Host never grants control access.
use crate::{Result, api::Api, fail, valid_id};
use axum::{
    Router,
    body::Body,
    extract::{Request, State},
    http::{Method, Response, StatusCode},
    response::IntoResponse,
};
use serde_json::Value;
use sqlx::Row;
use std::{path::Path, sync::Arc};
use tower::ServiceExt;
#[derive(Clone)]
pub struct Public {
    pub api: Arc<Api>,
    pub listen: std::net::SocketAddr,
}
pub fn router(state: Public) -> Router {
    Router::new().fallback(serve).with_state(state)
}
/// Preserve legacy3510 wiring: verified BFF Host remains private, tenant Hosts only serve apps.
pub fn combined(api: Arc<Api>, listen: std::net::SocketAddr) -> Router {
    let private = crate::api::router(api.clone());
    let public = router(Public { api, listen });
    Router::new().fallback(move |request: Request| {
        let private = private.clone();
        let public = public.clone();
        async move {
            let host = request
                .headers()
                .get("host")
                .and_then(|h| h.to_str().ok())
                .unwrap_or("")
                .split(':')
                .next()
                .unwrap_or("");
            if ["deploy.internal", "127.0.0.1", "localhost"].contains(&host) {
                private.oneshot(request).await
            } else {
                public.oneshot(request).await
            }
        }
    })
}

async fn serve(State(state): State<Public>, request: Request) -> Response<Body> {
    match dispatch(state, request).await {
        Ok(r) => r,
        Err(e) => e.into_response(),
    }
}
async fn resolve(state: &Public, host: &str) -> Result<Value> {
    let custom=sqlx::query("SELECT resource_type,project_id FROM deploy_store.custom_domains WHERE hostname=$1 AND dns_verified=1 AND tls_status='active'").bind(host).fetch_optional(&state.api.store.pool).await?;
    let effective = if let Some(row) = custom {
        let id: String = row.get("project_id");
        if row.get::<String, _>("resource_type") == "frontend" {
            let slug: Option<String> = sqlx::query_scalar(
                "SELECT slug FROM deploy_store.projects WHERE id=$1 AND deleted=0",
            )
            .bind(&id)
            .fetch_optional(&state.api.store.pool)
            .await?;
            format!(
                "{}.front.cbgpu.com",
                slug.ok_or_else(|| fail("No deployment found", 404))?
            )
        } else {
            let slug: Option<String> = sqlx::query_scalar(
                "SELECT slug FROM deploy_store.server_domains WHERE project_id=$1",
            )
            .bind(&id)
            .fetch_optional(&state.api.store.pool)
            .await?;
            format!(
                "{}.server.cbgpu.com",
                slug.unwrap_or_else(|| format!(
                    "server-{}",
                    id.chars().take(8).collect::<String>()
                ))
            )
        }
    } else {
        host.to_owned()
    };
    if let Some(slug) = effective.strip_suffix(".server.cbgpu.com") {
        if !regex::Regex::new(r"^[a-z][a-z0-9-]{2,34}$")
            .unwrap()
            .is_match(slug)
        {
            return Err(fail("No deployment found", 404));
        }
        let mut matches = Vec::new();
        for p in state.api.authority.server_projects().await? {
            if p["status"] != "approved" || p["access_scope"] != "public" {
                continue;
            }
            let Some(id) = p["id"].as_str().filter(|id| valid_id(id)) else {
                continue;
            };
            let mapped: Option<String> = sqlx::query_scalar(
                "SELECT slug FROM deploy_store.server_domains WHERE project_id=$1",
            )
            .bind(id)
            .fetch_optional(&state.api.store.pool)
            .await?;
            if mapped
                .unwrap_or_else(|| format!("server-{}", id.chars().take(8).collect::<String>()))
                == slug
            {
                matches.push(p);
            }
        }
        if matches.len() == 1 {
            let owner = matches[0]["user_id"]
                .as_str()
                .ok_or_else(|| fail("Server owner missing", 503))?;
            if !state.api.authority.can_execute(owner).await? {
                return Err(fail(
                    "School execution permission unavailable; project retained",
                    403,
                ));
            }
            let id = matches[0]["id"]
                .as_str()
                .ok_or_else(|| fail("Invalid server", 503))?;
            let access: Option<String> = sqlx::query_scalar(
                "SELECT access_state FROM deploy_store.compose_states WHERE project_id=$1",
            )
            .bind(id)
            .fetch_optional(&state.api.store.pool)
            .await?
            .flatten();
            if access.as_deref() == Some("DENY") {
                return Err(fail("Deployment access revoked; redeploy manually", 403));
            }
            return Ok(
                serde_json::json!({"kind":"server","port":matches[0]["host_port"],"_owner":owner,"_project":id}),
            );
        }
    } else if let Some(slug) = effective.strip_suffix(".front.cbgpu.com") {
        if !regex::Regex::new(r"^[a-z0-9-]+$").unwrap().is_match(slug) {
            return Err(fail("No deployment found", 404));
        }
        let mut row=sqlx::query("SELECT d.id,d.result,p.owner,p.id AS project_id FROM deploy_store.projects p JOIN deploy_store.deployments d ON d.id=p.production AND d.project_id=p.id WHERE p.slug=$1 AND p.deleted=0 AND d.status='ready'").bind(slug).fetch_optional(&state.api.store.pool).await?;
        if row.is_none()
            && let Some((base, prefix)) = slug.rsplit_once('-')
            && prefix.len() == 8
            && prefix.bytes().all(|b| b.is_ascii_hexdigit())
        {
            let rows=sqlx::query("SELECT d.id,d.result,p.owner,p.id AS project_id FROM deploy_store.projects p JOIN deploy_store.deployments d ON d.project_id=p.id WHERE p.slug=$1 AND p.deleted=0 AND d.status='ready'AND d.id LIKE $2").bind(base).bind(format!("{prefix}%")).fetch_all(&state.api.store.pool).await?;
            if rows.len() == 1 {
                row = rows.into_iter().next();
            }
        }
        if let Some(row) = row {
            let id: String = row.get("id");
            let mut value: Value = row.get("result");
            if !valid_id(&id) {
                return Err(fail("Invalid deployment identity", 503));
            }
            value["deployment_id"] = serde_json::json!(id);
            let owner: String = row.get("owner");
            let project_id: String = row.get("project_id");
            if value["kind"] == "next" && !state.api.authority.can_execute(&owner).await? {
                return Err(fail("Deployment execution access unavailable", 403));
            }
            value["_owner"] = serde_json::json!(owner);
            value["_project"] = serde_json::json!(project_id);
            return Ok(value);
        }
    }
    Err(fail("No deployment found for this domain", 404))
}
async fn dispatch(state: Public, mut request: Request) -> Result<Response<Body>> {
    if request.headers().get_all("host").iter().count() != 1 {
        return Err(fail("Invalid host", 400));
    }
    let host = request
        .headers()
        .get("host")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("")
        .split(':')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    if !regex::Regex::new(r"^[a-z0-9](?:[a-z0-9.-]*[a-z0-9])?$")
        .unwrap()
        .is_match(&host)
        || host.contains("..")
    {
        return Err(fail("Invalid host", 400));
    }
    let target = resolve(&state, &host).await?;
    if matches!(target["kind"].as_str(), Some("next" | "server")) {
        let port = target["port"]
            .as_u64()
            .filter(|p| (1024..=65535).contains(p))
            .ok_or_else(|| fail("Invalid deployment port", 503))? as u16;
        cbgpu_edge_policy::strip_request(request.headers_mut());
        request.headers_mut().insert(
            "x-forwarded-host",
            host.parse().map_err(|_| fail("Invalid host", 400))?,
        );
        request
            .headers_mut()
            .insert("x-forwarded-proto", "https".parse().unwrap());
        let expected = target.clone();
        let scope = state.clone();
        let hostname = host.clone();
        let check: crate::tenant_proxy::Check = std::sync::Arc::new(move || {
            let scope = scope.clone();
            let hostname = hostname.clone();
            let expected = expected.clone();
            Box::pin(async move {
                resolve(&scope, &hostname).await.is_ok_and(|current| {
                    current["port"] == expected["port"]
                        && current["_owner"] == expected["_owner"]
                        && current["_project"] == expected["_project"]
                        && current["deployment_id"] == expected["deployment_id"]
                })
            })
        });
        let mut response =
            crate::tenant_proxy::proxy(request, port, check, std::time::Duration::from_secs(5))
                .await?;
        cbgpu_edge_policy::filter_response(response.headers_mut(), &host);
        response
            .headers_mut()
            .insert("x-content-type-options", "nosniff".parse().unwrap());
        return Ok(response);
    }
    if !matches!(request.method(), &Method::GET | &Method::HEAD) {
        return Ok(StatusCode::METHOD_NOT_ALLOWED.into_response());
    }
    let id = target["deployment_id"]
        .as_str()
        .ok_or_else(|| fail("Invalid deployment", 503))?;
    let root = state.api.store.data.join(id).join("public");
    if target["public"].as_str().map(Path::new) != Some(root.as_path()) {
        return Err(fail("Static root ownership mismatch", 503));
    }
    let decoded = decode_path(request.uri().path())?;
    let file = static_file(&root, &decoded, target["spa"] == true)?;
    let size = file.metadata()?.len();
    let extension = Path::new(&decoded)
        .extension()
        .and_then(|v| v.to_str())
        .unwrap_or("");
    let content = mime(extension);
    let body = if request.method() == Method::HEAD {
        Body::empty()
    } else {
        let file = tokio::fs::File::from_std(file);
        Body::from_stream(futures_util::stream::try_unfold(
            file,
            |mut file| async move {
                use tokio::io::AsyncReadExt;
                let mut bytes = vec![0u8; 65536];
                let n = file.read(&mut bytes).await?;
                bytes.truncate(n);
                Ok::<_, std::io::Error>(if n == 0 { None } else { Some((bytes, file)) })
            },
        ))
    };
    Ok(Response::builder()
        .status(200)
        .header("content-type", content)
        .header("content-length", size)
        .header("cache-control", "public, max-age=0, must-revalidate")
        .header("x-content-type-options", "nosniff")
        .header("content-security-policy", "frame-ancestors 'self'")
        .header("strict-transport-security", "max-age=31536000")
        .body(body)
        .unwrap())
}
fn decode_path(path: &str) -> Result<String> {
    let mut out = Vec::new();
    let bytes = path.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            if i + 2 >= bytes.len() {
                return Err(fail("Invalid path", 400));
            }
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3])
                .ok()
                .and_then(|v| u8::from_str_radix(v, 16).ok())
                .ok_or_else(|| fail("Invalid path", 400))?;
            out.push(hex);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    let path = String::from_utf8(out).map_err(|_| fail("Invalid path", 400))?;
    if path.contains(['\0', '\\']) || path.split('/').any(|p| p.starts_with('.')) {
        return Err(fail("Not found", 404));
    }
    Ok(path)
}
fn static_file(root: &Path, name: &str, spa: bool) -> Result<std::fs::File> {
    use std::os::unix::{
        fs::OpenOptionsExt,
        io::{AsRawFd, FromRawFd},
    };
    let anchor = root
        .parent()
        .and_then(|p| p.parent())
        .ok_or_else(|| fail("Invalid static root", 503))?;
    if anchor.canonicalize().map_err(|_| fail("Not found", 404))? != anchor {
        return Err(fail("Invalid private release root", 503));
    }
    let mut base = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
        .open(anchor)
        .map_err(|_| fail("Not found", 404))?;
    for component in root
        .strip_prefix(anchor)
        .map_err(|_| fail("Invalid static root", 503))?
        .components()
    {
        let std::path::Component::Normal(part) = component else {
            return Err(fail("Invalid static root", 503));
        };
        use std::os::unix::ffi::OsStrExt;
        let name = std::ffi::CString::new(part.as_bytes())
            .map_err(|_| fail("Invalid static root", 503))?;
        let fd = unsafe {
            libc::openat(
                base.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
            )
        };
        if fd < 0 {
            return Err(fail("Not found", 404));
        }
        base = unsafe { std::fs::File::from_raw_fd(fd) };
    }
    let relative = name.trim_start_matches('/');
    let path = Path::new(relative);
    let mut candidates = vec![relative.to_owned(), format!("{relative}/index.html")];
    if relative.is_empty() {
        candidates.insert(0, "index.html".into());
    }
    if path.extension().is_none() {
        candidates.push(format!("{relative}.html"));
        if spa {
            candidates.push("index.html".into());
        }
    }
    for candidate in candidates {
        let parts: Vec<_> = candidate.split('/').filter(|p| !p.is_empty()).collect();
        if parts.is_empty() || parts.iter().any(|p| p.starts_with('.')) {
            continue;
        }
        let mut parent = base.try_clone()?;
        let mut found = None;
        for (i, part) in parts.iter().enumerate() {
            let Ok(name) = std::ffi::CString::new(*part) else {
                break;
            };
            let final_part = i + 1 == parts.len();
            let flags = libc::O_RDONLY
                | libc::O_CLOEXEC
                | libc::O_NOFOLLOW
                | libc::O_NONBLOCK
                | if final_part { 0 } else { libc::O_DIRECTORY };
            let fd = unsafe { libc::openat(parent.as_raw_fd(), name.as_ptr(), flags) };
            if fd < 0 {
                break;
            }
            let file = unsafe { std::fs::File::from_raw_fd(fd) };
            if final_part {
                if file.metadata()?.is_file() {
                    found = Some(file);
                }
            } else {
                parent = file;
            }
        }
        if let Some(file) = found {
            return Ok(file);
        }
    }
    Err(fail("Not found", 404))
}
fn mime(extension: &str) -> &'static str {
    match extension {
        "html" | "" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "json" => "application/json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "woff2" => "font/woff2",
        "txt" => "text/plain; charset=utf-8",
        "pdf" => "application/pdf",
        "wasm" => "application/wasm",
        _ => "application/octet-stream",
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn static_routes_refuse_traversal_symlinks_and_hidden_files() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("public");
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join("index.html"), "index").unwrap();
        std::fs::write(root.join("about.html"), "about").unwrap();
        assert!(static_file(&root, "/", false).is_ok());
        assert!(static_file(&root, "/about", false).is_ok());
        assert!(static_file(&root, "/client/route", true).is_ok());
        assert!(static_file(&root, "/missing.js", true).is_err());
        for path in ["/%2e%2e/secret", "/.env", "/%00", "/%5cetc", "/%zz"] {
            assert!(decode_path(path).is_err());
        }
        std::os::unix::fs::symlink(tmp.path(), root.join("escape")).unwrap();
        assert!(static_file(&root, "/escape/private", false).is_err());
        let linked = tmp.path().join("linked");
        std::os::unix::fs::symlink(&root, &linked).unwrap();
        assert!(static_file(&linked, "/", false).is_err());
    }
}
