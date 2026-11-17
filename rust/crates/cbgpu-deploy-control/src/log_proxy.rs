//! Read-only Docker UDS boundary for one Compose namespace; never mounts the real daemon socket.
use crate::{Result, fail, valid_id};
use axum::{
    Json, Router,
    body::{Body, to_bytes},
    extract::{Request, State},
    http::{Method, StatusCode},
    response::{IntoResponse, Response},
};
use hyper_util::rt::TokioIo;
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};
#[derive(Clone)]
struct Context {
    docker: PathBuf,
    namespace: String,
}
pub struct Proxy {
    task: tokio::task::JoinHandle<()>,
    socket: PathBuf,
}
impl Proxy {
    pub fn healthy(&self) -> bool {
        !self.task.is_finished()
    }
}
impl Drop for Proxy {
    fn drop(&mut self) {
        self.task.abort();
        let _ = std::fs::remove_file(&self.socket);
    }
}
pub async fn start(socket: &Path, docker: &Path, namespace: &str) -> Result<Proxy> {
    use std::os::unix::fs::{FileTypeExt, PermissionsExt};
    if !namespace
        .strip_prefix("cbgpu-compose-")
        .is_some_and(valid_id)
        || !socket.is_absolute()
        || !docker.is_absolute()
    {
        return Err(fail("Invalid private log proxy configuration", 503));
    }
    let parent = socket
        .parent()
        .ok_or_else(|| fail("Socket directory missing", 503))?;
    std::fs::create_dir_all(parent)?;
    std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))?;
    if let Ok(metadata) = std::fs::symlink_metadata(socket) {
        if !metadata.file_type().is_socket() {
            return Err(fail("Refusing to replace a non-socket file", 503));
        }
        if tokio::net::UnixStream::connect(socket).await.is_ok() {
            return Err(fail("Another controller still owns this log socket", 503));
        }
        std::fs::remove_file(socket)?;
    }
    let listener = tokio::net::UnixListener::bind(socket)?;
    std::fs::set_permissions(socket, std::fs::Permissions::from_mode(0o666))?;
    let context = Arc::new(Context {
        docker: docker.into(),
        namespace: namespace.into(),
    });
    let task = tokio::spawn(async move {
        let _ = axum::serve(listener, Router::new().fallback(handle).with_state(context)).await;
    });
    Ok(Proxy {
        task,
        socket: socket.into(),
    })
}
async fn request(
    socket: &Path,
    method: Method,
    path: &str,
) -> Result<hyper::Response<hyper::body::Incoming>> {
    let stream = tokio::time::timeout(
        std::time::Duration::from_secs(3),
        tokio::net::UnixStream::connect(socket),
    )
    .await
    .map_err(|_| fail("Docker log source unavailable", 503))??;
    let (mut sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
        .await
        .map_err(|_| fail("Docker log source unavailable", 503))?;
    tokio::spawn(async move {
        let _ = connection.await;
    });
    let request = hyper::Request::builder()
        .method(method)
        .uri(format!("http://docker{path}"))
        .header("host", "docker")
        .body(Body::empty())
        .map_err(|_| fail("Invalid Docker read path", 403))?;
    tokio::time::timeout(
        std::time::Duration::from_secs(10),
        sender.send_request(request),
    )
    .await
    .map_err(|_| fail("Docker log request timed out", 503))?
    .map_err(|_| fail("Docker log source unavailable", 503))
}
async fn value(context: &Context, path: &str) -> Result<Value> {
    let response = request(&context.docker, Method::GET, path).await?;
    if !response.status().is_success() {
        return Err(fail("Container unavailable", 403));
    }
    let bytes = to_bytes(Body::new(response.into_body()), 2 * 1024 * 1024)
        .await
        .map_err(|_| fail("Docker metadata exceeds its limit", 503))?;
    serde_json::from_slice(&bytes).map_err(|_| fail("Invalid Docker metadata", 503))
}
async fn handle(State(context): State<Arc<Context>>, request: Request) -> Response {
    match allowed(&context, request).await {
        Ok(response) => response,
        Err(_) => StatusCode::FORBIDDEN.into_response(),
    }
}
async fn allowed(context: &Context, incoming: Request) -> Result<Response> {
    if ![Method::GET, Method::HEAD].contains(incoming.method()) {
        return Err(fail("Read-only Docker log boundary", 403));
    }
    let method = incoming.method().clone();
    let mut url = url::Url::parse(&format!(
        "http://docker{}",
        incoming
            .uri()
            .path_and_query()
            .map(|v| v.as_str())
            .unwrap_or("/")
    ))
    .map_err(|_| fail("Invalid Docker path", 403))?;
    let version = regex::Regex::new(r"^/v[0-9]+(?:\.[0-9]+)*").unwrap();
    let path = version.replace(url.path(), "").into_owned();
    url.set_path(&path);
    if path == "/containers/json" {
        let filters = json!({"label":[format!("com.docker.compose.project={}",context.namespace)]})
            .to_string();
        let query = url::form_urlencoded::Serializer::new(String::new())
            .append_pair("all", "true")
            .append_pair("filters", &filters)
            .finish();
        let data = value(context, &format!("/containers/json?{query}")).await?;
        let rows: Vec<_> = data
            .as_array()
            .ok_or_else(|| fail("Invalid container list", 503))?
            .iter()
            .filter(|v| v["Labels"]["com.docker.compose.project"] == context.namespace)
            .cloned()
            .collect();
        return Ok(Json(rows).into_response());
    }
    if path == "/events" {
        let pairs: Vec<_> = url
            .query_pairs()
            .filter(|(k, _)| k != "filters")
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect();
        url.query_pairs_mut()
            .clear()
            .extend_pairs(pairs)
            .append_pair(
                "filters",
                &json!({"label":[format!("com.docker.compose.project={}",context.namespace)]})
                    .to_string(),
            );
    } else if !["/_ping", "/version"].contains(&path.as_str()) {
        let pieces: Vec<_> = path.trim_start_matches('/').split('/').collect();
        if pieces.len() != 3
            || pieces[0] != "containers"
            || !["json", "logs"].contains(&pieces[2])
            || pieces[1].is_empty()
            || pieces[1].len() > 128
            || !pieces[1]
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
        {
            return Err(fail("Docker endpoint denied", 403));
        }
        let inspected = value(context, &format!("/containers/{}/json", pieces[1])).await?;
        if inspected["Config"]["Labels"]["com.docker.compose.project"] != context.namespace {
            return Err(fail("Container belongs to another project", 403));
        }
        let actual = inspected["Id"]
            .as_str()
            .filter(|id| {
                !id.is_empty() && id.len() <= 64 && id.bytes().all(|b| b.is_ascii_hexdigit())
            })
            .ok_or_else(|| fail("Invalid immutable container identity", 503))?;
        if pieces[2] == "json" {
            return Ok(Json(inspected).into_response());
        }
        url.set_path(&format!("/containers/{actual}/logs"));
    }
    let target = format!(
        "{}{}",
        url.path(),
        url.query().map(|v| format!("?{v}")).unwrap_or_default()
    );
    let response = request(&context.docker, method, &target).await?;
    let (status, headers) = (response.status(), response.headers().clone());
    let mut result = Response::builder()
        .status(status)
        .body(Body::new(response.into_body()))
        .map_err(|_| fail("Docker response unavailable", 503))?;
    for (name, value) in &headers {
        if !["connection", "upgrade", "set-cookie", "transfer-encoding"].contains(&name.as_str()) {
            result.headers_mut().append(name, value.clone());
        }
    }
    Ok(result)
}
#[cfg(test)]
mod tests {
    use super::*;
    use axum::routing::{any, get};
    #[tokio::test]
    async fn namespace_filter_blocks_foreign_reads_and_every_write_without_forwarding_credentials()
    {
        let temp = tempfile::tempdir().unwrap();
        let docker = temp.path().join("docker.sock");
        let socket = temp.path().join("project.sock");
        let ns = "cbgpu-compose-10000000-0000-4000-8000-000000000001";
        let app=Router::new().route("/containers/json",get(move||async move{Json(json!([{"Id":"aa","Labels":{"com.docker.compose.project":ns}},{"Id":"bb","Labels":{"com.docker.compose.project":"other"}}]))})).route("/containers/own/json",get(move||async move{Json(json!({"Id":"aa","Config":{"Labels":{"com.docker.compose.project":ns}}}))})).route("/containers/foreign/json",get(||async{Json(json!({"Id":"bb","Config":{"Labels":{"com.docker.compose.project":"other"}}}))})).route("/containers/aa/logs",any(|req:Request|async move{assert_eq!(req.method(),Method::GET);assert!(req.headers().get("authorization").is_none());assert!(req.headers().get("cookie").is_none());"owned logs"})).route("/events",get(move|req:Request|async move{let url=url::Url::parse(&format!("http://docker{}",req.uri().path_and_query().unwrap())).unwrap();let filters:Vec<_>=url.query_pairs().filter(|(k,_)|k=="filters").collect();assert_eq!(filters.len(),1);assert!(filters[0].1.contains(ns));assert!(!filters[0].1.contains("foreign"));"owned event"}));
        let listener = tokio::net::UnixListener::bind(&docker).unwrap();
        let daemon = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let _proxy = start(&socket, &docker, ns).await.unwrap();
        let response = request(&socket, Method::GET, "/v1.44/containers/json")
            .await
            .unwrap();
        let bytes = to_bytes(Body::new(response.into_body()), 10000)
            .await
            .unwrap();
        let values: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(values.as_array().unwrap().len(), 1);
        for (method, path) in [
            (Method::POST, "/containers/create"),
            (Method::GET, "/containers/foreign/logs"),
            (Method::GET, "/containers/aa/archive"),
            (Method::GET, "/info"),
        ] {
            assert_eq!(request(&socket, method, path).await.unwrap().status(), 403)
        }
        let response = request(
            &socket,
            Method::GET,
            "/containers/own/logs?stdout=true&follow=true",
        )
        .await
        .unwrap();
        assert_eq!(response.status(), 200);
        assert_eq!(
            to_bytes(Body::new(response.into_body()), 10000)
                .await
                .unwrap(),
            "owned logs"
        );
        assert_eq!(
            request(
                &socket,
                Method::GET,
                "/events?filters=foreign&filters=other"
            )
            .await
            .unwrap()
            .status(),
            200
        );
        daemon.abort();
    }
}
