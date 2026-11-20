use axum::{
    body::to_bytes,
    extract::Request,
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use rand::{RngCore, rngs::OsRng};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Identity {
    pub allowed: bool,
    pub user_id: String,
    pub role: String,
}
#[async_trait::async_trait]
pub trait Authority: Send + Sync {
    async fn verify(&self, token: &str) -> Result<Identity, StatusCode>;
}
pub struct RemoteAuthority {
    pub url: String,
    pub host: String,
    pub client: reqwest::Client,
}
#[async_trait::async_trait]
impl Authority for RemoteAuthority {
    async fn verify(&self, token: &str) -> Result<Identity, StatusCode> {
        let r = self
            .client
            .get(&self.url)
            .header("host", &self.host)
            .bearer_auth(token)
            .timeout(Duration::from_secs(5))
            .send()
            .await
            .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
        if !r.status().is_success() {
            return Err(match r.status().as_u16() {
                401 => StatusCode::UNAUTHORIZED,
                403 | 404 => StatusCode::FORBIDDEN,
                _ => StatusCode::SERVICE_UNAVAILABLE,
            });
        }
        let value: Identity = r
            .json()
            .await
            .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
        if !value.allowed
            || !matches!(value.role.as_str(), "owner" | "editor")
            || value.user_id.len() != 36
        {
            return Err(StatusCode::FORBIDDEN);
        }
        Ok(value)
    }
}
pub fn random() -> String {
    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}
pub fn opaque(s: &str) -> bool {
    s.len() == 43
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}
pub struct Challenge {
    pub nonce: String,
    pub path: String,
    pub until: Instant,
    pub issued: bool,
}
pub struct Ticket {
    pub challenge: String,
    pub token: String,
    pub user: String,
    pub until: Instant,
}
#[derive(Clone)]
pub struct Session {
    pub token: String,
    pub user: String,
    pub until: Instant,
}
#[derive(Default)]
pub struct Book {
    pub challenges: HashMap<String, Challenge>,
    pub tickets: HashMap<String, Ticket>,
    pub sessions: HashMap<String, Session>,
    pub rate: HashMap<String, (Instant, u32)>,
}
impl Book {
    fn clean(&mut self) {
        let now = Instant::now();
        self.challenges.retain(|_, v| v.until > now);
        self.tickets.retain(|_, v| v.until > now);
        self.sessions.retain(|_, v| v.until > now);
        self.rate
            .retain(|_, v| now.duration_since(v.0) < Duration::from_secs(60));
    }
    fn capacity(&mut self, bucket: &str) -> bool {
        self.clean();
        let value = self
            .rate
            .entry(bucket.to_string())
            .or_insert((Instant::now(), 0));
        value.1 += 1;
        value.1 <= 60
            && self.challenges.len() < 256
            && self.tickets.len() < 256
            && self.sessions.len() < 128
    }
}
pub struct Auth {
    pub authority: Arc<dyn Authority>,
    pub book: Mutex<Book>,
    pub host: String,
    pub project: String,
    pub checks: tokio::sync::Semaphore,
}
pub fn response(status: StatusCode, text: &str) -> Response {
    let mut r = (status, text.to_string()).into_response();
    r.headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    r.headers_mut().insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    r
}
fn cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    let mut values = headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|h| h.to_str().ok())
        .flat_map(|s| s.split(';'))
        .filter_map(|s| s.trim().split_once('='))
        .filter(|(key, _)| *key == name)
        .map(|(_, v)| v.to_string());
    let value = values.next()?;
    if values.next().is_some() || !opaque(&value) {
        None
    } else {
        Some(value)
    }
}
fn set_cookie(r: &mut Response, name: &str, value: &str, seconds: u64) {
    r.headers_mut().append(
        header::SET_COOKIE,
        HeaderValue::from_str(&format!(
            "{name}={value}; Path=/; Secure; HttpOnly; SameSite=Lax; Max-Age={seconds}"
        ))
        .unwrap(),
    );
}
fn redirect(location: &str) -> Response {
    let mut r = response(StatusCode::SEE_OTHER, "");
    r.headers_mut()
        .insert(header::LOCATION, HeaderValue::from_str(location).unwrap());
    r
}
fn safe_path(path: &str) -> String {
    if path.len() <= 2048
        && path.starts_with('/')
        && !path.starts_with("//")
        && !path.contains(['\\', '\r', '\n'])
        && !["%5c", "%2f", "%0d", "%0a", "%00"]
            .iter()
            .any(|v| path.to_ascii_lowercase().contains(v))
    {
        path.to_string()
    } else {
        "/".to_string()
    }
}
impl Auth {
    pub async fn verify(&self, token: &str) -> Result<Identity, StatusCode> {
        let _permit = tokio::time::timeout(Duration::from_secs(1), self.checks.acquire())
            .await
            .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?
            .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
        let identity = self.authority.verify(token).await?;
        if !identity.allowed
            || !matches!(identity.role.as_str(), "owner" | "editor")
            || identity.user_id.len() != 36
        {
            return Err(StatusCode::FORBIDDEN);
        }
        Ok(identity)
    }

    pub fn login(&self, headers: &HeaderMap, path: &str) -> Response {
        let mut book = self.book.lock().unwrap();
        if !book.capacity("login") {
            return response(StatusCode::TOO_MANY_REQUESTS, "잠시 후 다시 접속하세요.");
        };
        let challenge = random();
        let nonce = random();
        book.challenges.insert(
            challenge.clone(),
            Challenge {
                nonce: nonce.clone(),
                path: safe_path(path),
                until: Instant::now() + Duration::from_secs(300),
                issued: false,
            },
        );
        let app_referrer = headers
            .get(header::REFERER)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| url::Url::parse(v).ok())
            .is_some_and(|v| {
                v.username().is_empty()
                    && v.password().is_none()
                    && v.origin().ascii_serialization() == "https://app.cbgpu.com"
            });
        let webview = headers
            .get(header::USER_AGENT)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.contains("; wv)") || v.contains("CBGPUApp"));
        let first_party = if app_referrer || webview {
            "https://app.cbgpu.com"
        } else {
            "https://cbgpu.com"
        };
        let mut r = redirect(&format!(
            "{first_party}/server/{}/open?challenge={challenge}",
            self.project
        ));
        set_cookie(&mut r, "__Host-studio-nonce", &nonce, 300);
        r
    }
    pub async fn issue(&self, request: Request) -> Response {
        if request
            .headers()
            .get(header::ORIGIN)
            .is_some_and(|v| v != "https://cbgpu.com" && v != "https://app.cbgpu.com")
        {
            return response(StatusCode::FORBIDDEN, "접근할 수 없습니다.");
        }
        let token = match request
            .headers()
            .get(header::AUTHORIZATION)
            .and_then(|h| h.to_str().ok())
            .and_then(|s| s.strip_prefix("Bearer "))
        {
            Some(v) if !v.is_empty() && v.len() <= 8192 && !v.contains(char::is_whitespace) => {
                v.to_string()
            }
            _ => return response(StatusCode::UNAUTHORIZED, "로그인이 필요합니다."),
        };
        let body = match to_bytes(request.into_body(), 2048).await {
            Ok(v) => v,
            Err(_) => return response(StatusCode::BAD_REQUEST, "잘못된 요청입니다."),
        };
        let challenge = match serde_json::from_slice::<serde_json::Value>(&body)
            .ok()
            .and_then(|v| {
                v.get("challenge")
                    .and_then(|s| s.as_str())
                    .map(str::to_string)
            }) {
            Some(v) if opaque(&v) => v,
            _ => return response(StatusCode::BAD_REQUEST, "잘못된 요청입니다."),
        };
        {
            let mut b = self.book.lock().unwrap();
            if !b.capacity("issue") {
                return response(StatusCode::TOO_MANY_REQUESTS, "잠시 후 다시 시도하세요.");
            };
            if !b.challenges.get(&challenge).is_some_and(|c| !c.issued) {
                return response(StatusCode::BAD_REQUEST, "다시 열어 주세요.");
            }
        }
        let identity = match self.verify(&token).await {
            Ok(v) => v,
            Err(code) => return response(code, "접근 권한을 확인하세요."),
        };
        let mut b = self.book.lock().unwrap();
        b.clean();
        let Some(c) = b.challenges.get_mut(&challenge) else {
            return response(StatusCode::BAD_REQUEST, "다시 열어 주세요.");
        };
        if c.issued {
            return response(StatusCode::BAD_REQUEST, "다시 열어 주세요.");
        };
        c.issued = true;
        let ticket = random();
        b.tickets.insert(
            ticket.clone(),
            Ticket {
                challenge,
                token,
                user: identity.user_id,
                until: Instant::now() + Duration::from_secs(30),
            },
        );
        let mut r = response(
            StatusCode::OK,
            &serde_json::json!({"ticket":ticket}).to_string(),
        );
        r.headers_mut().insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
        r
    }
    pub fn redeem(&self, request: &Request) -> Response {
        let query: Vec<_> =
            url::form_urlencoded::parse(request.uri().query().unwrap_or("").as_bytes()).collect();
        if query.len() != 1 || query[0].0 != "ticket" || !opaque(&query[0].1) {
            return response(StatusCode::BAD_REQUEST, "다시 열어 주세요.");
        };
        let ticket = query[0].1.to_string();
        let Some(nonce) = cookie(request.headers(), "__Host-studio-nonce") else {
            return response(StatusCode::FORBIDDEN, "이 브라우저에서 다시 열어 주세요.");
        };
        let mut b = self.book.lock().unwrap();
        b.clean();
        let Some(t) = b.tickets.get(&ticket) else {
            return response(StatusCode::FORBIDDEN, "다시 열어 주세요.");
        };
        let Some(c) = b.challenges.get(&t.challenge) else {
            return response(StatusCode::FORBIDDEN, "다시 열어 주세요.");
        };
        use subtle::ConstantTimeEq;
        if !bool::from(c.nonce.as_bytes().ct_eq(nonce.as_bytes())) {
            return response(StatusCode::FORBIDDEN, "이 브라우저에서 다시 열어 주세요.");
        };
        if b.sessions.len() >= 128 {
            return response(StatusCode::SERVICE_UNAVAILABLE, "잠시 후 다시 시도하세요.");
        };
        let t = b.tickets.remove(&ticket).unwrap();
        let c = b.challenges.remove(&t.challenge).unwrap();
        let key = random();
        b.sessions.insert(
            key.clone(),
            Session {
                token: t.token,
                user: t.user,
                until: Instant::now() + Duration::from_secs(3600),
            },
        );
        let mut r = redirect(&c.path);
        set_cookie(&mut r, "__Host-studio-session", &key, 3600);
        set_cookie(&mut r, "__Host-studio-nonce", "", 0);
        r
    }
    pub async fn authenticate(
        &self,
        headers: &HeaderMap,
        method: &axum::http::Method,
        path: &str,
    ) -> Result<Session, Box<Response>> {
        let key = cookie(headers, "__Host-studio-session");
        let session = {
            let mut b = self.book.lock().unwrap();
            b.clean();
            key.as_ref().and_then(|key| b.sessions.get(key)).cloned()
        };
        let Some(session) = session else {
            return Err(Box::new(if *method == axum::http::Method::GET {
                self.login(headers, path)
            } else {
                response(StatusCode::UNAUTHORIZED, "로그인이 필요합니다.")
            }));
        };
        let identity = self.verify(&session.token).await;
        match identity {
            Ok(v) if v.user_id == session.user => Ok(session),
            other => {
                if !matches!(other, Err(StatusCode::SERVICE_UNAVAILABLE)) {
                    self.book
                        .lock()
                        .unwrap()
                        .sessions
                        .remove(key.as_ref().unwrap());
                }
                let code = match other {
                    Err(code) => code,
                    _ => StatusCode::FORBIDDEN,
                };
                let mut r = response(code, "로그인 및 접근 권한을 확인하세요.");
                if code != StatusCode::SERVICE_UNAVAILABLE {
                    set_cookie(&mut r, "__Host-studio-session", "", 0)
                }
                Err(Box::new(r))
            }
        }
    }
}
