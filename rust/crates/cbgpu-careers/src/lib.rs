//! Careers API replacement. Tokens and assets remain compatible with the Python service.
//! No public endpoint lists applicants; operator reads require database credentials.
pub mod import;
pub mod outbox;
use axum::{
    Router,
    body::{Body, to_bytes},
    extract::{ConnectInfo, Request, State},
    http::{HeaderValue, Method, StatusCode, header},
    response::{IntoResponse, Response},
};
use chrono::{TimeZone, Utc};
use hmac::{Hmac, Mac};
use rand::RngCore;
use serde_json::{Value, json};
use sha2::Sha256;
use sqlx::{PgPool, Row};
use std::{collections::BTreeMap, net::SocketAddr, path::PathBuf, sync::Arc};

const INVALID: &str = "지원서 형식을 확인해주세요.";
type Hmac256 = Hmac<Sha256>;
#[derive(Clone)]
pub struct App {
    pub pool: PgPool,
    pub key: Arc<Vec<u8>>,
    pub roles: Arc<Value>,
    pub public: PathBuf,
    pub origin: String,
    pub release: String,
}
#[derive(Debug)]
pub struct Application {
    pub role: String,
    pub digest: String,
    pub fields: BTreeMap<String, String>,
}
fn signature(key: &[u8], value: &str) -> String {
    let mut mac = Hmac256::new_from_slice(key).expect("HMAC accepts any key size");
    mac.update(value.as_bytes());
    hex::encode(mac.finalize().into_bytes())
}
pub fn issue_token(key: &[u8], now: i64) -> String {
    let mut nonce = [0_u8; 16];
    rand::thread_rng().fill_bytes(&mut nonce);
    let value = format!("{now}.{}", hex::encode(nonce));
    format!("{value}.{}", signature(key, &value))
}
pub fn valid_token(key: &[u8], token: &str, now: i64) -> bool {
    let parts: Vec<_> = token.split('.').collect();
    if parts.len() != 3
        || parts[1].len() != 32
        || !parts[1]
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return false;
    }
    let Ok(stamp) = parts[0].parse::<i64>() else {
        return false;
    };
    if !(0..7200).contains(&now.saturating_sub(stamp)) {
        return false;
    }
    if parts[2].len() != 64
        || !parts[2]
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return false;
    }
    let Ok(digest) = hex::decode(parts[2]) else {
        return false;
    };
    let mut mac = Hmac256::new_from_slice(key).expect("HMAC accepts any key size");
    mac.update(format!("{}.{}", parts[0], parts[1]).as_bytes());
    mac.verify_slice(&digest).is_ok()
}
fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(v) => *v,
        Value::String(v) => !v.is_empty(),
        Value::Array(v) => !v.is_empty(),
        Value::Object(v) => !v.is_empty(),
        Value::Number(v) => v.as_f64() != Some(0.0),
    }
}
pub fn validate(app: &App, data: &Value, now: i64) -> Result<Application, &'static str> {
    if !data.is_object() {
        return Err(INVALID);
    }
    let role = data["role"].as_str().unwrap_or("");
    if !app
        .roles
        .as_array()
        .is_some_and(|roles| roles.iter().any(|r| r["id"] == role))
    {
        return Err("지원 분야를 확인해주세요.");
    }
    if data["consent"] != true {
        return Err("개인정보 수집·이용 동의가 필요해요.");
    }
    let token = data["token"].as_str().unwrap_or("");
    if !valid_token(&app.key, token, now) {
        return Err("지원 화면이 만료됐어요. 내용을 복사한 후 페이지를 새로고침해주세요.");
    }
    if truthy(&data["website"]) {
        return Err("자동 접수는 허용하지 않아요.");
    }
    let mut fields = BTreeMap::new();
    for (name, limit) in [
        ("name", 60),
        ("email", 180),
        ("affiliation", 100),
        ("motivation", 4000),
        ("experience", 4000),
        ("portfolio", 500),
    ] {
        let value = match data.get(name) {
            None => "",
            Some(Value::String(value)) => value,
            _ => return Err("입력 내용의 길이나 형식을 확인해주세요."),
        };
        if value.chars().count() > limit || value.contains('\0') {
            return Err("입력 내용의 길이나 형식을 확인해주세요.");
        }
        fields.insert(name.to_owned(), value.trim().to_owned());
    }
    let email = &fields["email"];
    let email_valid = email.split_once('@').is_some_and(|(left, right)| {
        !left.is_empty()
            && !right.contains('@')
            && right
                .rsplit_once('.')
                .is_some_and(|(host, tld)| !host.is_empty() && !tld.is_empty())
    }) && !email.chars().any(char::is_whitespace);
    if fields["name"].is_empty() || !email_valid || fields["motivation"].chars().count() < 20 {
        return Err("이름·이메일과 20자 이상의 지원 동기를 확인해주세요.");
    }
    if !fields["portfolio"].is_empty()
        && !url::Url::parse(&fields["portfolio"]).is_ok_and(|url| {
            ["http", "https"].contains(&url.scheme())
                && url.host_str().is_some()
                && url.username().is_empty()
                && url.password().is_none()
        })
    {
        return Err("포트폴리오는 http 또는 https 링크로 입력해주세요.");
    }
    Ok(Application {
        role: role.to_owned(),
        digest: signature(&app.key, token),
        fields,
    })
}
fn response(status: StatusCode, body: impl Into<Body>, kind: &'static str) -> Response {
    let mut response = Response::new(body.into());
    *response.status_mut() = status;
    for (name, value) in [
        ("content-type", kind),
        ("cache-control", "no-store"),
        ("x-content-type-options", "nosniff"),
        ("referrer-policy", "strict-origin-when-cross-origin"),
        ("x-frame-options", "DENY"),
        (
            "permissions-policy",
            "camera=(), microphone=(), geolocation=()",
        ),
        (
            "content-security-policy",
            "default-src 'self'; script-src 'self'; style-src 'self' https://fonts.googleapis.com; font-src 'self' https://fonts.gstatic.com; img-src 'self' data:; connect-src 'self'; object-src 'none'; base-uri 'self'; frame-ancestors 'none'; form-action 'self'",
        ),
        ("strict-transport-security", "max-age=31536000"),
    ] {
        response
            .headers_mut()
            .insert(name, HeaderValue::from_static(value));
    }
    response
}
fn json_response(status: StatusCode, value: Value) -> Response {
    response(status, value.to_string(), "application/json; charset=utf-8")
}
fn error(status: StatusCode, message: &str) -> Response {
    json_response(status, json!({"error":message}))
}
async fn store(
    app: &App,
    application: &Application,
    ip: &str,
    now: i64,
) -> Result<Response, sqlx::Error> {
    let mut tx = app.pool.begin().await?;
    // All submitters take token then IP locks in a fixed order. Separate namespaces
    // prevent cross-kind collisions. Transaction locks survive concurrent retries.
    sqlx::query("SELECT pg_advisory_xact_lock(1,hashtext($1))")
        .bind(&application.digest)
        .execute(&mut *tx)
        .await?;
    if let Some(row) = sqlx::query("SELECT id FROM careers.applications WHERE token_hash=$1")
        .bind(&application.digest)
        .fetch_optional(&mut *tx)
        .await?
    {
        let reference: String = row.try_get("id")?;
        return Ok(json_response(
            StatusCode::OK,
            json!({"reference":reference}),
        ));
    }
    let ip = signature(&app.key, ip);
    sqlx::query("SELECT pg_advisory_xact_lock(2,hashtext($1))")
        .bind(&ip)
        .execute(&mut *tx)
        .await?;
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM careers.requests WHERE ip=$1 AND created>$2")
            .bind(&ip)
            .bind(now - 3600)
            .fetch_one(&mut *tx)
            .await?;
    if count >= 5 {
        let mut response = error(
            StatusCode::TOO_MANY_REQUESTS,
            "접수 요청이 많아요. 한 시간 뒤 다시 시도해주세요.",
        );
        response
            .headers_mut()
            .insert("retry-after", HeaderValue::from_static("3600"));
        return Ok(response);
    }
    let mut random = [0_u8; 4];
    rand::thread_rng().fill_bytes(&mut random);
    let date = chrono::FixedOffset::east_opt(9 * 3600)
        .expect("KST")
        .timestamp_opt(now, 0)
        .single()
        .expect("validated current timestamp")
        .format("%y%m%d");
    let reference = format!("CB-{date}-{}", hex::encode_upper(random));
    let fields = &application.fields;
    sqlx::query("INSERT INTO careers.applications(id,token_hash,created,role,name,email,affiliation,motivation,experience,portfolio,consent_version) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,'2026-11-04')")
        .bind(&reference).bind(&application.digest).bind(now).bind(&application.role).bind(&fields["name"]).bind(&fields["email"]).bind(&fields["affiliation"]).bind(&fields["motivation"]).bind(&fields["experience"]).bind(&fields["portfolio"]).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO careers.requests(ip,created) VALUES($1,$2)")
        .bind(ip)
        .bind(now)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(json_response(
        StatusCode::CREATED,
        json!({"reference":reference}),
    ))
}
fn html_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#x27;")
}
async fn handle(State(app): State<App>, request: Request) -> Response {
    let path = request.uri().path().to_owned();
    if request.method() == Method::POST {
        if path != "/api/applications" {
            return error(StatusCode::NOT_FOUND, "Not found");
        }
        if request
            .headers()
            .get(header::ORIGIN)
            .and_then(|v| v.to_str().ok())
            != Some(app.origin.as_str())
        {
            return error(StatusCode::FORBIDDEN, "모집 사이트에서 직접 제출해주세요.");
        }
        if request
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.split(';').next())
            .map(str::trim)
            != Some("application/json")
        {
            return error(StatusCode::UNSUPPORTED_MEDIA_TYPE, "JSON 요청이 필요해요.");
        }
        let length = request
            .headers()
            .get(header::CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(0);
        if !(1..=30000).contains(&length) {
            return error(StatusCode::PAYLOAD_TOO_LARGE, "지원서 크기가 너무 커요.");
        }
        let ip = request
            .headers()
            .get("cf-connecting-ip")
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned)
            .or_else(|| {
                request
                    .extensions()
                    .get::<ConnectInfo<SocketAddr>>()
                    .map(|v| v.0.ip().to_string())
            })
            .unwrap_or_else(|| "unknown".into());
        let bytes = match tokio::time::timeout(
            std::time::Duration::from_secs(15),
            to_bytes(request.into_body(), 30000),
        )
        .await
        {
            Ok(Ok(bytes)) if bytes.len() == length => bytes,
            _ => return error(StatusCode::BAD_REQUEST, INVALID),
        };
        let data: Value = match serde_json::from_slice(&bytes) {
            Ok(data) => data,
            Err(_) => return error(StatusCode::BAD_REQUEST, INVALID),
        };
        let now = Utc::now().timestamp();
        let application = match validate(&app, &data, now) {
            Ok(value) => value,
            Err(message) => return error(StatusCode::BAD_REQUEST, message),
        };
        return match store(&app, &application, &ip, now).await {
            Ok(response) => response,
            Err(_) => error(
                StatusCode::SERVICE_UNAVAILABLE,
                "접수를 완료하지 못했어요. 잠시 후 다시 시도해주세요.",
            ),
        };
    }
    if request.method() != Method::GET {
        return StatusCode::METHOD_NOT_ALLOWED.into_response();
    }
    match path.as_str() {
        "/health" => {
            return json_response(StatusCode::OK, json!({"status":"ok","commit":app.release}));
        }
        "/api/roles" => return json_response(StatusCode::OK, (*app.roles).clone()),
        "/api/form" => {
            return json_response(
                StatusCode::OK,
                json!({"token":issue_token(&app.key,Utc::now().timestamp())}),
            );
        }
        "/roles/mobile" | "/apply/mobile" => {
            let mut r = response(StatusCode::FOUND, "", "application/json; charset=utf-8");
            r.headers_mut()
                .insert(header::LOCATION, HeaderValue::from_static("/#positions"));
            return r;
        }
        "/robots.txt" => {
            return response(
                StatusCode::OK,
                format!(
                    "User-agent: *\nDisallow: /api/\nDisallow: /apply/\nDisallow: /onboarding\nSitemap: {}/sitemap.xml\n",
                    app.origin
                ),
                "text/plain; charset=utf-8",
            );
        }
        "/sitemap.xml" => {
            let mut paths = vec!["/".to_owned()];
            for role in app.roles.as_array().into_iter().flatten() {
                if let Some(id) = role["id"].as_str() {
                    paths.push(format!("/roles/{id}"));
                }
            }
            let entries = paths
                .into_iter()
                .map(|p| format!("<url><loc>{}{}</loc></url>", html_escape(&app.origin), p))
                .collect::<String>();
            return response(
                StatusCode::OK,
                format!(
                    "<?xml version=\"1.0\" encoding=\"UTF-8\"?><urlset xmlns=\"http://www.sitemaps.org/schemas/sitemap/0.9\">{entries}</urlset>"
                ),
                "application/xml",
            );
        }
        _ => {}
    }
    let asset = match path.as_str() {
        "/style.css" => Some(("style.css", "text/css; charset=utf-8")),
        "/theme.js" => Some(("theme.js", "text/javascript; charset=utf-8")),
        "/app.js" => Some(("app.js", "text/javascript; charset=utf-8")),
        "/onboarding.js" => Some(("onboarding.js", "text/javascript; charset=utf-8")),
        "/logo.png" => Some(("logo.png", "image/png")),
        _ => None,
    };
    if let Some((file, kind)) = asset {
        return match tokio::fs::read(app.public.join(file)).await {
            Ok(bytes) => response(StatusCode::OK, bytes, kind),
            Err(_) => error(StatusCode::SERVICE_UNAVAILABLE, "Page unavailable"),
        };
    }
    let role = path
        .strip_prefix("/roles/")
        .or_else(|| path.strip_prefix("/apply/"))
        .and_then(|id| {
            app.roles
                .as_array()?
                .iter()
                .find(|r| r["id"] == id.strip_suffix('/').unwrap_or(id))
        });
    if ["/", "/privacy", "/onboarding"].contains(&path.as_str()) || role.is_some() {
        let file = if path == "/onboarding" {
            "onboarding.html"
        } else {
            "index.html"
        };
        let Ok(mut page) = tokio::fs::read_to_string(app.public.join(file)).await else {
            return error(StatusCode::SERVICE_UNAVAILABLE, "Page unavailable");
        };
        if let Some(role) = role.filter(|_| path.starts_with("/roles/") && !path.ends_with('/')) {
            page = page.replace(
                "CBGPU Careers — 학교의 일상을 바꾸는 팀",
                &format!(
                    "{} | CBGPU Careers",
                    html_escape(role["title"].as_str().unwrap_or(""))
                ),
            );
        }
        return response(StatusCode::OK, page, "text/html; charset=utf-8");
    }
    error(StatusCode::NOT_FOUND, "찾을 수 없는 페이지예요.")
}
pub fn router(app: App) -> Router {
    Router::new().fallback(handle).with_state(app)
}
