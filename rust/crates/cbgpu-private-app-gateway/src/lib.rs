//! Trusted HOST gate for a single private local-only application. User credentials
//! remain in this process and never enter the application's container.
pub mod auth;
use auth::{Auth, Session, response};
use axum::{
    Router,
    body::Body,
    extract::{Request, State},
    http::{HeaderMap, HeaderValue, Method, StatusCode, header},
    response::Response,
};
use futures_util::StreamExt;
use std::{sync::Arc, time::Duration};
pub struct Gateway {
    pub auth: Arc<Auth>,
    pub listen: String,
    pub upstream: String,
    pub app_authority: String,
    pub client: reqwest::Client,
}
fn hop(name: &str) -> bool {
    matches!(
        name,
        "connection"
            | "keep-alive"
            | "proxy-authenticate"
            | "proxy-authorization"
            | "te"
            | "trailer"
            | "transfer-encoding"
            | "upgrade"
    )
}
fn connection_headers(headers: &HeaderMap) -> std::collections::HashSet<String> {
    headers
        .get(header::CONNECTION)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .split(',')
        .map(|s| s.trim().to_ascii_lowercase())
        .collect()
}
async fn still_allowed(auth: &Auth, session: &Session) -> bool {
    session.until > std::time::Instant::now()
        && auth
            .verify(&session.token)
            .await
            .is_ok_and(|v| v.user_id == session.user)
}
async fn proxy(State(g): State<Arc<Gateway>>, request: Request) -> Response {
    let host = request
        .headers()
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();
    if request.headers().get_all(header::HOST).iter().count() != 1 {
        return response(StatusCode::BAD_REQUEST, "잘못된 주소입니다.");
    }
    let ticket_path = request.uri().path() == "/_cbgpu/auth/tickets";
    if host != g.auth.host && !(ticket_path && host == g.listen) {
        return response(StatusCode::FORBIDDEN, "접근할 수 없습니다.");
    }
    if request.headers().contains_key(header::UPGRADE) {
        return response(
            StatusCode::UPGRADE_REQUIRED,
            "이 앱은 웹소켓을 지원하지 않습니다.",
        );
    }
    if ticket_path {
        return if request.method() == Method::POST {
            g.auth.issue(request).await
        } else {
            response(StatusCode::METHOD_NOT_ALLOWED, "잘못된 요청입니다.")
        };
    }
    if request.uri().path() == "/_cbgpu/auth/redeem" {
        return if request.method() == Method::GET {
            g.auth.redeem(&request)
        } else {
            response(StatusCode::METHOD_NOT_ALLOWED, "잘못된 요청입니다.")
        };
    }
    if request.uri().path().starts_with("/_cbgpu/") {
        return response(StatusCode::NOT_FOUND, "요청을 찾을 수 없습니다.");
    }
    let session = match g
        .auth
        .authenticate(
            request.headers(),
            request.method(),
            request
                .uri()
                .path_and_query()
                .map(|v| v.as_str())
                .unwrap_or("/"),
        )
        .await
    {
        Ok(s) => s,
        Err(r) => return *r,
    };
    if !matches!(*request.method(), Method::GET | Method::HEAD) {
        let expected = format!("https://{}", g.auth.host);
        if request.headers().get_all(header::ORIGIN).iter().count() != 1
            || request
                .headers()
                .get(header::ORIGIN)
                .and_then(|v| v.to_str().ok())
                != Some(&expected)
            || request
                .headers()
                .get("sec-fetch-site")
                .is_some_and(|v| v == "cross-site")
        {
            return response(StatusCode::FORBIDDEN, "현재 작업공간에서 요청하세요.");
        }
    }
    let url = format!(
        "http://{}{}",
        g.upstream,
        request
            .uri()
            .path_and_query()
            .map(|v| v.as_str())
            .unwrap_or("/")
    );
    let (parts, body) = request.into_parts();
    let skip = connection_headers(&parts.headers);
    let mut outgoing = g.client.request(parts.method, url);
    let mut boundary_headers = parts.headers.clone();
    cbgpu_edge_policy::strip_request(&mut boundary_headers);
    for (name, value) in &boundary_headers {
        if hop(name.as_str())
            || skip.contains(name.as_str())
            || matches!(
                name.as_str(),
                "host" | "authorization" | "origin" | "cookie" | "forwarded"
            )
            || name.as_str().starts_with("x-forwarded-")
            || name.as_str().starts_with("x-cbgpu-")
        {
            continue;
        }
        outgoing = outgoing.header(name, value);
    }
    outgoing = outgoing.header(header::HOST, &g.app_authority);
    if parts.headers.contains_key(header::ORIGIN) {
        outgoing = outgoing.header(header::ORIGIN, format!("http://{}", g.app_authority));
    }
    let send = outgoing
        .body(reqwest::Body::wrap_stream(body.into_data_stream()))
        .send();
    tokio::pin!(send);
    let mut interval = tokio::time::interval(Duration::from_secs(5));
    interval.tick().await;
    let upstream = loop {
        tokio::select! {result=&mut send=>{break match result{Ok(v)=>v,Err(_)=>return response(StatusCode::BAD_GATEWAY,"앱이 시작 중입니다. 잠시 후 다시 접속하세요.")}},_=interval.tick()=>{if !still_allowed(&g.auth,&session).await{return response(StatusCode::UNAUTHORIZED,"로그인 상태를 확인하세요.")}}}
    };
    let status = upstream.status();
    let skip = connection_headers(upstream.headers());
    let mut headers = HeaderMap::new();
    for (name, value) in upstream.headers() {
        if !hop(name.as_str()) && !skip.contains(name.as_str()) && name != header::SET_COOKIE {
            headers.append(
                name,
                if name == header::LOCATION {
                    public_location(value, &g.app_authority, &g.auth.host)
                } else {
                    value.clone()
                },
            );
        }
    }
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    let data = Box::pin(upstream.bytes_stream());
    let auth = g.auth.clone();
    let stream = futures_util::stream::unfold(
        (data, interval, auth, session, false),
        |(mut data, mut interval, auth, session, done)| async move {
            if done {
                return None;
            }
            loop {
                tokio::select! {chunk=data.next()=>{return chunk.map(|v|(v.map_err(std::io::Error::other),(data,interval,auth,session,false)))},_=interval.tick()=>{if !still_allowed(&auth,&session).await{return Some((Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied,"access expired")),(data,interval,auth,session,true)))}}}
            }
        },
    );
    let mut r = Response::new(Body::from_stream(stream));
    *r.status_mut() = status;
    *r.headers_mut() = headers;
    r
}
fn public_location(value: &HeaderValue, app_authority: &str, public_host: &str) -> HeaderValue {
    let Some(url) = value.to_str().ok().and_then(|v| url::Url::parse(v).ok()) else {
        return value.clone();
    };
    let authority = &url[url::Position::BeforeHost..url::Position::AfterPort];
    if matches!(url.scheme(), "http" | "https")
        && url.username().is_empty()
        && url.password().is_none()
        && authority == app_authority
    {
        HeaderValue::from_str(&format!(
            "https://{public_host}{}",
            &url[url::Position::BeforePath..]
        ))
        .unwrap_or_else(|_| value.clone())
    } else {
        value.clone()
    }
}
pub fn router(g: Arc<Gateway>) -> Router {
    Router::new().fallback(proxy).with_state(g)
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::{Authority, Book, Identity};
    use axum::response::IntoResponse;
    use std::sync::{
        Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    };
    struct Fake {
        active: AtomicBool,
        role: Mutex<String>,
        calls: AtomicUsize,
    }
    #[async_trait::async_trait]
    impl Authority for Fake {
        async fn verify(&self, token: &str) -> Result<Identity, StatusCode> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if token != "fixture-owner" || !self.active.load(Ordering::SeqCst) {
                return Err(StatusCode::UNAUTHORIZED);
            }
            Ok(Identity {
                allowed: true,
                user_id: "11111111-2222-4333-8444-555555555555".into(),
                role: self.role.lock().unwrap().clone(),
            })
        }
    }
    struct Fixture {
        url: String,
        g: Arc<Gateway>,
        fake: Arc<Fake>,
        tasks: Vec<tokio::task::JoinHandle<()>>,
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            for task in &self.tasks {
                task.abort()
            }
        }
    }
    async fn fixture() -> Fixture {
        let upstream = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let upstream_addr = upstream.local_addr().unwrap();
        let app = Router::new().fallback(|request: Request| async move {
            if request.uri().path() == "/stream" {
                let stream = futures_util::stream::unfold(0, |i| async move {
                    tokio::time::sleep(Duration::from_millis(500)).await;
                    Some((Ok::<_, std::io::Error>(format!("data: {i}\n\n")), i + 1))
                });
                return Response::new(Body::from_stream(stream));
            }
            let headers = request.headers().clone();
            let data = axum::body::to_bytes(request.into_body(), 3 * 1024 * 1024)
                .await
                .unwrap();
            let mut r = Response::new(Body::from(format!(
                "host={};origin={};auth={};cookie={};client={};bytes={}",
                headers.get("host").unwrap().to_str().unwrap(),
                headers
                    .get("origin")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("none"),
                headers.contains_key("authorization"),
                headers.contains_key("cookie"),
                headers
                    .get("x-backspace-client")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("none"),
                data.len()
            )));
            r.headers_mut().append(
                header::SET_COOKIE,
                HeaderValue::from_static(
                    "__Host-studio-session=attacker; Path=/; Secure; HttpOnly",
                ),
            );
            r
        });
        let app_task = tokio::spawn(async move { axum::serve(upstream, app).await.unwrap() });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let fake = Arc::new(Fake {
            active: AtomicBool::new(true),
            role: Mutex::new("owner".into()),
            calls: AtomicUsize::new(0),
        });
        let auth = Arc::new(Auth {
            authority: fake.clone(),
            book: Mutex::new(Book::default()),
            host: "workspace.server.cbgpu.com".into(),
            project: "11111111-2222-4333-8444-555555555555".into(),
            checks: tokio::sync::Semaphore::new(32),
        });
        let g = Arc::new(Gateway {
            auth,
            listen: addr.to_string(),
            upstream: upstream_addr.to_string(),
            app_authority: "127.0.0.1:8787".into(),
            client: reqwest::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .unwrap(),
        });
        let state = g.clone();
        let task = tokio::spawn(async move { axum::serve(listener, router(state)).await.unwrap() });
        Fixture {
            url: format!("http://{addr}"),
            g,
            fake,
            tasks: vec![task, app_task],
        }
    }
    fn client() -> reqwest::Client {
        reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap()
    }
    fn cookie_value(r: &reqwest::Response, name: &str) -> String {
        r.headers()
            .get_all(header::SET_COOKIE)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .find_map(|v| {
                v.strip_prefix(&format!("{name}="))
                    .map(|v| v.split(';').next().unwrap().to_string())
            })
            .unwrap()
    }
    async fn login(f: &Fixture) -> (String, String) {
        let r = client()
            .get(format!("{}/?mode=workspace", f.url))
            .header("host", &f.g.auth.host)
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 303);
        assert!(
            r.headers()[header::LOCATION]
                .to_str()
                .unwrap()
                .starts_with("https://cbgpu.com/server/")
        );
        let nonce = cookie_value(&r, "__Host-studio-nonce");
        let url = url::Url::parse(r.headers()[header::LOCATION].to_str().unwrap()).unwrap();
        let challenge = url
            .query_pairs()
            .find(|(k, _)| k == "challenge")
            .unwrap()
            .1
            .to_string();
        (challenge, nonce)
    }
    async fn ticket(f: &Fixture, challenge: &str) -> reqwest::Response {
        client()
            .post(format!("{}/_cbgpu/auth/tickets", f.url))
            .header("host", &f.g.listen)
            .bearer_auth("fixture-owner")
            .json(&serde_json::json!({"challenge":challenge}))
            .send()
            .await
            .unwrap()
    }
    async fn authorized(f: &Fixture) -> String {
        let (challenge, nonce) = login(f).await;
        let r = ticket(f, &challenge).await;
        assert_eq!(r.status(), 200);
        let t = r.json::<serde_json::Value>().await.unwrap()["ticket"]
            .as_str()
            .unwrap()
            .to_string();
        let r = client()
            .get(format!("{}/_cbgpu/auth/redeem?ticket={t}", f.url))
            .header("host", &f.g.auth.host)
            .header("cookie", format!("__Host-studio-nonce={nonce}"))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 303);
        assert_eq!(r.headers()[header::LOCATION], "/?mode=workspace");
        cookie_value(&r, "__Host-studio-session")
    }
    #[tokio::test]
    async fn bound_nonce_single_use_and_safe_cookie() {
        let f = fixture().await;
        let (challenge, nonce) = login(&f).await;
        let r = ticket(&f, &challenge).await;
        let t = r.json::<serde_json::Value>().await.unwrap()["ticket"]
            .as_str()
            .unwrap()
            .to_string();
        let url = format!("{}/_cbgpu/auth/redeem?ticket={t}", f.url);
        let r = client()
            .get(&url)
            .header("host", &f.g.auth.host)
            .header("cookie", format!("__Host-studio-nonce={}", auth::random()))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 403);
        let r = client()
            .get(&url)
            .header("host", &f.g.auth.host)
            .header("cookie", format!("__Host-studio-nonce={nonce}"))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 303);
        for h in r.headers().get_all(header::SET_COOKIE) {
            let h = h.to_str().unwrap();
            assert!(h.contains("Secure; HttpOnly; SameSite=Lax"));
            assert!(!h.contains("Domain="));
            assert!(!h.contains("fixture-owner"));
        }
        assert_eq!(r.headers()[header::REFERRER_POLICY], "no-referrer");
        assert_eq!(
            client()
                .get(&url)
                .header("host", &f.g.auth.host)
                .header("cookie", format!("__Host-studio-nonce={nonce}"))
                .send()
                .await
                .unwrap()
                .status(),
            403
        );
    }
    #[tokio::test]
    async fn upload_boundary_csrf_revocation_and_viewer() {
        let f = fixture().await;
        let cookie = authorized(&f).await;
        let request = || {
            client()
                .post(format!("{}/upload", f.url))
                .header("host", &f.g.auth.host)
                .header(
                    "cookie",
                    format!("__Host-studio-session={cookie}; sb-cbgpu-auth-token=private"),
                )
                .header("authorization", "Bearer never-forward")
                .header("x-backspace-client", "web")
        };
        assert_eq!(request().send().await.unwrap().status(), 403);
        assert_eq!(
            request()
                .header("origin", "https://sibling.cbgpu.com")
                .send()
                .await
                .unwrap()
                .status(),
            403
        );
        let r = request()
            .header("origin", format!("https://{}", f.g.auth.host))
            .body(vec![b'x'; 2 * 1024 * 1024])
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 200);
        assert!(!r.headers().contains_key(header::SET_COOKIE));
        let data = r.text().await.unwrap();
        assert!(data.contains("host=127.0.0.1:8787;origin=http://127.0.0.1:8787;auth=false;cookie=false;client=web;bytes=2097152"));
        *f.fake.role.lock().unwrap() = "viewer".into();
        assert_eq!(
            request()
                .header("origin", format!("https://{}", f.g.auth.host))
                .send()
                .await
                .unwrap()
                .status(),
            403
        );
        let (c, _) = login(&f).await;
        assert_eq!(ticket(&f, &c).await.status(), 403);
    }
    #[tokio::test]
    async fn unauth_mutation_never_replayed_and_context_is_fixed() {
        let f = fixture().await;
        assert_eq!(
            client()
                .post(&f.url)
                .header("host", &f.g.auth.host)
                .send()
                .await
                .unwrap()
                .status(),
            401
        );
        for (referer, expected) in [
            ("https://attacker.example/", "https://cbgpu.com/"),
            (
                "https://app.cbgpu.com.attacker.example/",
                "https://cbgpu.com/",
            ),
            ("https://app.cbgpu.com/server/x", "https://app.cbgpu.com/"),
        ] {
            let r = client()
                .get(&f.url)
                .header("host", &f.g.auth.host)
                .header("referer", referer)
                .send()
                .await
                .unwrap();
            assert!(
                r.headers()[header::LOCATION]
                    .to_str()
                    .unwrap()
                    .starts_with(expected)
            );
        }
        let r = client()
            .get(&f.url)
            .header("host", &f.g.auth.host)
            .header("user-agent", "Android; wv) CBGPUApp")
            .send()
            .await
            .unwrap();
        assert!(
            r.headers()[header::LOCATION]
                .to_str()
                .unwrap()
                .starts_with("https://app.cbgpu.com/")
        );
    }
    #[tokio::test]
    async fn active_stream_closes_after_revocation() {
        let f = fixture().await;
        let cookie = authorized(&f).await;
        let r = client()
            .get(format!("{}/stream", f.url))
            .header("host", &f.g.auth.host)
            .header("cookie", format!("__Host-studio-session={cookie}"))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 200);
        let mut stream = r.bytes_stream();
        assert!(stream.next().await.unwrap().is_ok());
        f.fake.active.store(false, Ordering::SeqCst);
        let ended = tokio::time::timeout(Duration::from_secs(7), async {
            while let Some(part) = stream.next().await {
                if part.is_err() {
                    break;
                }
            }
        })
        .await;
        assert!(ended.is_ok());
        assert!(f.fake.calls.load(Ordering::SeqCst) >= 3);
    }
    #[tokio::test]
    async fn expiry_rate_bounds_and_editor_access() {
        let f = fixture().await;
        *f.fake.role.lock().unwrap() = "editor".into();
        let session = authorized(&f).await;
        for entry in f.g.auth.book.lock().unwrap().sessions.values_mut() {
            entry.until = std::time::Instant::now() - Duration::from_secs(1);
        }
        let r = client()
            .get(&f.url)
            .header("host", &f.g.auth.host)
            .header("cookie", format!("__Host-studio-session={session}"))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 303);
        let (challenge, _) = login(&f).await;
        f.g.auth
            .book
            .lock()
            .unwrap()
            .challenges
            .get_mut(&challenge)
            .unwrap()
            .until = std::time::Instant::now() - Duration::from_secs(1);
        assert_eq!(ticket(&f, &challenge).await.status(), 400);
        let (challenge, nonce) = login(&f).await;
        let t = ticket(&f, &challenge)
            .await
            .json::<serde_json::Value>()
            .await
            .unwrap()["ticket"]
            .as_str()
            .unwrap()
            .to_string();
        f.g.auth
            .book
            .lock()
            .unwrap()
            .tickets
            .get_mut(&t)
            .unwrap()
            .until = std::time::Instant::now() - Duration::from_secs(1);
        assert_eq!(
            client()
                .get(format!("{}/_cbgpu/auth/redeem?ticket={t}", f.url))
                .header("host", &f.g.auth.host)
                .header("cookie", format!("__Host-studio-nonce={nonce}"))
                .send()
                .await
                .unwrap()
                .status(),
            403
        );
        let limited = fixture().await;
        for _ in 0..60 {
            assert_eq!(
                client()
                    .get(&limited.url)
                    .header("host", &limited.g.auth.host)
                    .send()
                    .await
                    .unwrap()
                    .status(),
                303
            );
        }
        assert_eq!(
            client()
                .get(&limited.url)
                .header("host", &limited.g.auth.host)
                .send()
                .await
                .unwrap()
                .status(),
            429
        );
        assert_eq!(limited.g.auth.book.lock().unwrap().challenges.len(), 60);
    }
    #[tokio::test]
    async fn first_party_authority_uses_real_http_and_fails_closed() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app=Router::new().fallback(|request:Request|async move {
            assert_eq!(request.headers()[header::HOST],"cbgpu.com");
            if request.headers().get(header::AUTHORIZATION).is_some_and(|v|v=="Bearer synthetic-owner") {
                (StatusCode::OK, axum::Json(serde_json::json!({"allowed":true,"userId":"11111111-2222-4333-8444-555555555555","role":"owner"}))).into_response()
            } else { StatusCode::SERVICE_UNAVAILABLE.into_response() }
        });
        let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let authority = auth::RemoteAuthority {
            url: format!("http://{addr}/api/server/fixture/open"),
            host: "cbgpu.com".into(),
            client: client(),
        };
        assert!(authority.verify("synthetic-owner").await.unwrap().allowed);
        assert!(matches!(
            authority.verify("synthetic-bad").await,
            Err(StatusCode::SERVICE_UNAVAILABLE)
        ));
        task.abort();
    }
    #[test]
    fn local_redirects_preserve_public_origin_without_touching_external_oauth() {
        let local = HeaderValue::from_static("http://127.0.0.1:8787/auth/callback?code=fixture");
        assert_eq!(
            public_location(&local, "127.0.0.1:8787", "workspace.server.cbgpu.com"),
            "https://workspace.server.cbgpu.com/auth/callback?code=fixture"
        );
        for target in [
            "https://accounts.example/authorize?state=fixture",
            "/relative",
            "http://127.0.0.1:9999/other",
        ] {
            let value = HeaderValue::from_str(target).unwrap();
            assert_eq!(
                public_location(&value, "127.0.0.1:8787", "workspace.server.cbgpu.com"),
                value
            );
        }
    }
}
