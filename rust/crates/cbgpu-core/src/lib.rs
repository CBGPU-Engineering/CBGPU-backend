use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
};
use chrono::{DateTime, Datelike, FixedOffset, Timelike, Utc, Weekday};
use futures_util::StreamExt;
use serde_json::{Value, json};
use sqlx::{PgPool, Row, postgres::PgPoolOptions};
use std::{
    net::{IpAddr, SocketAddr},
    path::PathBuf,
    sync::Arc,
    time::Duration,
};
use subtle::ConstantTimeEq;
use uuid::Uuid;

pub const MAX_RESPONSE_BYTES: usize = 2 * 1024 * 1024;
#[derive(Clone)]
pub struct Settings {
    pub database_url: String,
    pub listen: SocketAddr,
    pub web_origin: url::Url,
    pub cron_secret_file: PathBuf,
    pub admin_key_file: PathBuf,
    pub timetable_secret_file: PathBuf,
    pub timetable_school_id: String,
    pub release: String,
}
impl Settings {
    pub fn from_env() -> Result<Self, &'static str> {
        let database_url = std::env::var("CBGPU_RUNTIME_DATABASE_URL")
            .map_err(|_| "Runtime database URL is not configured")?;
        let listen = std::env::var("CBGPU_RUST_LISTEN")
            .unwrap_or_else(|_| "127.0.0.1:3540".into())
            .parse::<SocketAddr>()
            .map_err(|_| "Invalid listen address")?;
        if !listen.ip().is_loopback() {
            return Err("Rust core must bind to loopback");
        }
        let web_origin = parse_web_origin(
            &std::env::var("CBGPU_WEB_ORIGIN").unwrap_or_else(|_| "http://127.0.0.1:3000".into()),
        )?;
        let home = std::env::var("HOME").map_err(|_| "HOME is not configured")?;
        Ok(Self {
            database_url,
            listen,
            web_origin,
            cron_secret_file: std::env::var_os("CBGPU_STUDY_SECRET_FILE")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from(&home).join(".config/cbgpu/study-auto-secret")),
            timetable_secret_file: std::env::var_os("CBGPU_TIMETABLE_WARM_SECRET_FILE")
                .map(PathBuf::from)
                .unwrap_or_else(|| {
                    PathBuf::from(&home).join(".config/cbgpu/timetable-warm-secret")
                }),
            timetable_school_id: std::env::var("CBGPU_TIMETABLE_SCHOOL_ID")
                .unwrap_or_else(|_| "cbsh".into()),
            admin_key_file: std::env::var_os("CBGPU_RUST_ADMIN_KEY_FILE")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from(home).join(".config/cbgpu/rust-core-admin-key")),
            release: std::env::var("CBGPU_RELEASE_SHA").unwrap_or_else(|_| "development".into()),
        })
    }
}
pub fn parse_web_origin(value: &str) -> Result<url::Url, &'static str> {
    let url = url::Url::parse(value).map_err(|_| "Invalid local web origin")?;
    let host = url.host_str().and_then(|s| s.parse::<IpAddr>().ok());
    if url.scheme() != "http"
        || !host.is_some_and(|ip| ip.is_loopback())
        || !url.username().is_empty()
        || url.password().is_some()
        || url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err("Web upstream must be a numeric loopback HTTP origin");
    }
    Ok(url)
}
pub async fn pool(database_url: &str, max: u32) -> Result<PgPool, sqlx::Error> {
    PgPoolOptions::new()
        .max_connections(max)
        .min_connections(0)
        .acquire_timeout(Duration::from_secs(5))
        .idle_timeout(Duration::from_secs(60))
        .connect(database_url)
        .await
}
pub fn study_window(at: DateTime<Utc>) -> bool {
    let local = at.with_timezone(&FixedOffset::east_opt(9 * 3600).expect("KST offset"));
    !matches!(local.weekday(), Weekday::Sat | Weekday::Sun)
        && (450..=930).contains(&(local.hour() * 60 + local.minute()))
}
pub fn slot(operation: &str, at: DateTime<Utc>) -> i64 {
    at.timestamp()
        / if operation == "timetable-refresh" {
            300
        } else {
            60
        }
}
pub async fn private_file(path: &std::path::Path) -> Result<String, &'static str> {
    let metadata = tokio::fs::symlink_metadata(path)
        .await
        .map_err(|_| "Private credential file unavailable")?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err("Credential must be a regular file");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err("Credential file must use mode 600");
        }
    }
    let value = tokio::fs::read_to_string(path)
        .await
        .map_err(|_| "Credential cannot be read")?;
    let value = value.trim();
    if value.len() < 16 || value.len() > 4096 || value.contains(['\n', '\r']) {
        return Err("Invalid credential format");
    }
    Ok(value.to_owned())
}
#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub key: Arc<str>,
    pub release: Arc<str>,
}
fn reply(status: StatusCode, value: Value) -> Response {
    let mut response = (status, Json(value)).into_response();
    response
        .headers_mut()
        .insert("cache-control", "no-store".parse().expect("static header"));
    response.headers_mut().insert(
        "x-content-type-options",
        "nosniff".parse().expect("static header"),
    );
    response
}
fn authorized(headers: &HeaderMap, key: &str) -> bool {
    headers
        .get("authorization")
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer "))
        .is_some_and(|value| value.as_bytes().ct_eq(key.as_bytes()).into())
}
async fn health(State(state): State<AppState>) -> Response {
    reply(
        StatusCode::OK,
        json!({"status":"ok","implementation":"rust-axum","commit":state.release.as_ref()}),
    )
}
async fn ready(State(state): State<AppState>) -> Response {
    let result = tokio::time::timeout(
        Duration::from_secs(3),
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM operations.runs WHERE status='running'")
            .fetch_one(&state.pool),
    )
    .await;
    if matches!(result, Ok(Ok(_))) {
        reply(
            StatusCode::OK,
            json!({"status":"ok","database":"postgresql"}),
        )
    } else {
        reply(
            StatusCode::SERVICE_UNAVAILABLE,
            json!({"status":"unavailable"}),
        )
    }
}
async fn operations(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if !authorized(&headers, &state.key) {
        return reply(
            StatusCode::UNAUTHORIZED,
            json!({"message":"Authentication required"}),
        );
    }
    match sqlx::query("SELECT operation,status,started_at::text AS started_at,finished_at::text AS finished_at,result FROM operations.runs ORDER BY started_at DESC LIMIT 50").fetch_all(&state.pool).await {
        Ok(rows)=>reply(StatusCode::OK,json!({"runs":rows.iter().map(|r|json!({"operation":r.get::<String,_>("operation"),"status":r.get::<String,_>("status"),"started_at":r.get::<String,_>("started_at"),"finished_at":r.get::<Option<String>,_>("finished_at"),"result":r.get::<Value,_>("result")})).collect::<Vec<_>>()})),
        Err(_)=>reply(StatusCode::SERVICE_UNAVAILABLE,json!({"message":"Operations unavailable"})),
    }
}
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/health/ready", get(ready))
        .route("/internal/operations", get(operations))
        .with_state(state)
}

pub fn summarize(operation: &str, data: &Value) -> Result<Value, &'static str> {
    if operation == "study-auto" {
        if data.get("ok").and_then(Value::as_bool) != Some(true) {
            return Err("Scheduler did not confirm success");
        }
        Ok(
            json!({"ok":true,"processed":data.get("processed").and_then(Value::as_u64).unwrap_or(0),"skipped":data.get("skipped").and_then(Value::as_bool)}),
        )
    } else {
        if data.get("ok").and_then(Value::as_bool) != Some(true)
            || data.get("stale").and_then(Value::as_bool) != Some(false)
            || data.get("cells").and_then(Value::as_u64).unwrap_or(0) == 0
            || data.get("days").and_then(Value::as_u64).unwrap_or(0) == 0
        {
            return Err("Fresh timetable warm was not confirmed");
        }
        Ok(json!({"ok":true,"stale":false,"days":data["days"],"cells":data["cells"]}))
    }
}
pub async fn invoke(
    client: &reqwest::Client,
    settings: &Settings,
    operation: &str,
) -> Result<Value, &'static str> {
    let path = if operation == "study-auto" {
        "/api/cron/study-auto"
    } else {
        "/api/cron/timetable-warm"
    };
    let url = settings
        .web_origin
        .join(path)
        .map_err(|_| "Invalid upstream path")?;
    let mut request = if operation == "study-auto" {
        client.post(url).json(&json!({}))
    } else {
        if settings.timetable_school_id.is_empty()
            || settings.timetable_school_id.len() > 100
            || !settings
                .timetable_school_id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        {
            return Err("Invalid operator timetable school");
        }
        client
            .post(url)
            .json(&json!({"school_id":settings.timetable_school_id,"grade":1,"class":1}))
            .header(
                "x-cbgpu-timetable-warm",
                private_file(&settings.timetable_secret_file).await?,
            )
    };
    request = request
        .header("host", "cbgpu.com")
        .header("user-agent", "CBGPU-RustOperations/0.1");
    if operation == "study-auto" {
        request = request.bearer_auth(private_file(&settings.cron_secret_file).await?);
    }
    let response = request
        .send()
        .await
        .map_err(|_| "Upstream transport failed; request outcome may be uncertain")?;
    if !response.status().is_success() {
        return Err("Upstream rejected the operation");
    }
    let mut body = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| "Upstream response was interrupted")?;
        if body.len() + chunk.len() > MAX_RESPONSE_BYTES {
            return Err("Upstream response exceeded the limit");
        }
        body.extend_from_slice(&chunk);
    }
    let data: Value =
        serde_json::from_slice(&body).map_err(|_| "Upstream response was not valid JSON")?;
    summarize(operation, &data)
}
pub async fn run(
    settings: &Settings,
    operation: &str,
    at: DateTime<Utc>,
    dry_run: bool,
) -> Result<Value, &'static str> {
    if !["study-auto", "timetable-refresh"].contains(&operation) {
        return Err("Unsupported operation");
    }
    if operation == "study-auto" && !study_window(at) {
        return Ok(json!({"ok":true,"skipped":"outside_weekday_window"}));
    }
    let pool = pool(&settings.database_url, 1)
        .await
        .map_err(|_| "Runtime database unavailable; existing server data is unchanged")?;
    if dry_run {
        if operation == "study-auto" {
            private_file(&settings.cron_secret_file).await?;
        }
        sqlx::query("SELECT 1 FROM operations.runs LIMIT 1")
            .execute(&pool)
            .await
            .map_err(|_| "Runtime schema unavailable")?;
        pool.close().await;
        return Ok(json!({"ok":true,"dry_run":true,"operation":operation,"writes":false}));
    }
    let id = Uuid::new_v4();
    let inserted=sqlx::query("INSERT INTO operations.runs(id,operation,slot,status) VALUES($1,$2,$3,'running') ON CONFLICT(operation,slot) DO NOTHING RETURNING id").bind(id).bind(operation).bind(slot(operation,at)).fetch_optional(&pool).await.map_err(|_|"Cannot claim operation")?;
    if inserted.is_none() {
        pool.close().await;
        return Ok(json!({"ok":true,"skipped":"slot_already_claimed"}));
    }
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(if operation == "study-auto" {
            150
        } else {
            100
        }))
        .build()
        .map_err(|_| "Cannot initialize HTTP client")?;
    // Deliberately one request. Never retry an ambiguous school write from this transport.
    let result = invoke(&client, settings, operation).await;
    let (status, report) = match &result {
        Ok(value) => ("succeeded", value.clone()),
        Err(message) => (
            if operation == "study-auto" {
                "uncertain"
            } else {
                "failed"
            },
            json!({"ok":false,"reason":message}),
        ),
    };
    sqlx::query("UPDATE operations.runs SET status=$1,finished_at=now(),result=$2 WHERE id=$3")
        .bind(status)
        .bind(report)
        .bind(id)
        .execute(&pool)
        .await
        .map_err(|_| "Operation finished but audit acknowledgement failed; do not retry blindly")?;
    pool.close().await;
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    fn at(value: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(value)
            .unwrap()
            .with_timezone(&Utc)
    }
    #[test]
    fn weekday_window_matches_legacy_inclusive_boundaries() {
        for (time, expected) in [
            ("2026-11-15T22:29:00Z", false),
            ("2026-11-15T22:30:00Z", true),
            ("2026-11-16T06:30:59Z", true),
            ("2026-11-16T06:31:00Z", false),
            ("2026-11-14T00:00:00Z", false),
            ("2026-11-15T00:00:00Z", false),
        ] {
            assert_eq!(study_window(at(time)), expected, "{time}");
        }
    }
    #[test]
    fn only_numeric_loopback_upstreams() {
        for value in [
            "https://cbgpu.com",
            "http://localhost:3000",
            "http://127.0.0.1:3000/private",
            "http://u:p@127.0.0.1:3000",
            "http://127.0.0.1:3000/?q=1",
            "http://192.168.1.1:3000",
        ] {
            assert!(parse_web_origin(value).is_err(), "{value}");
        }
        assert!(parse_web_origin("http://127.0.0.1:3000").is_ok());
    }
    #[test]
    fn do_not_record_student_or_timetable_contents() {
        let value = summarize(
            "study-auto",
            &json!({"ok":true,"processed":2,"token":"secret","student":{"name":"private"}}),
        )
        .unwrap();
        assert!(!value.to_string().contains("private"));
        assert!(!value.to_string().contains("secret"));
        assert!(summarize("timetable-refresh", &json!({"grid":[],"stale":false})).is_err());
        assert!(summarize("timetable-refresh", &json!({"grid":[[1]],"stale":true})).is_err());
        assert!(summarize("study-auto", &json!({"ok":false})).is_err());
    }
    #[test]
    fn work_slots_are_stable() {
        let a = at("2026-11-16T01:30:00Z");
        assert_eq!(
            slot("study-auto", a),
            slot("study-auto", a + chrono::Duration::seconds(59))
        );
        assert_ne!(
            slot("study-auto", a),
            slot("study-auto", a + chrono::Duration::seconds(60))
        );
    }
    #[tokio::test]
    async fn health_does_not_require_database_and_admin_route_denies_anonymous() {
        use axum::{body::Body, http::Request};
        use tower::ServiceExt;
        let pool = PgPoolOptions::new()
            .connect_lazy("postgres://unused:unused@127.0.0.1:1/unused")
            .unwrap();
        let app = router(AppState {
            pool,
            key: Arc::from("test-only-key-long"),
            release: Arc::from("test"),
        });
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/internal/operations")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }
    #[tokio::test]
    async fn timetable_warm_uses_dedicated_secret_and_never_records_payload() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let key = dir.path().join("warm-key");
        std::fs::write(&key, "a".repeat(64)).unwrap();
        std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o600)).unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app=Router::new().route("/api/cron/timetable-warm",axum::routing::post(|headers:HeaderMap,Json(body):Json<Value>|async move {
            assert_eq!(headers.get("x-cbgpu-timetable-warm").unwrap(),"a".repeat(64).as_str());assert!(headers.get("authorization").is_none());
            assert_eq!(body,json!({"school_id":"school-b","grade":1,"class":1}));
            Json(json!({"ok":true,"stale":false,"days":5,"cells":35,"grid":{"private":"not recorded"}}))
        }));
        let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let settings = Settings {
            database_url: String::new(),
            listen: addr,
            web_origin: parse_web_origin(&format!("http://{addr}")).unwrap(),
            cron_secret_file: PathBuf::new(),
            admin_key_file: PathBuf::new(),
            timetable_secret_file: key,
            timetable_school_id: "school-b".into(),
            release: String::new(),
        };
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap();
        let result = invoke(&client, &settings, "timetable-refresh")
            .await
            .unwrap();
        assert_eq!(result, json!({"ok":true,"stale":false,"days":5,"cells":35}));
        task.abort();
    }
    #[tokio::test]
    async fn http_contract_rejects_redirects_and_malformed_responses() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = Router::new().route(
            "/api/cron/timetable-warm",
            axum::routing::post(|| async {
                axum::response::Redirect::temporary("https://example.invalid/")
            }),
        );
        let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let dir = tempfile::tempdir().unwrap();
        let secret = dir.path().join("secret");
        std::fs::write(&secret, "a".repeat(64)).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&secret, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        let settings = Settings {
            database_url: String::new(),
            listen: addr,
            web_origin: parse_web_origin(&format!("http://{addr}")).unwrap(),
            cron_secret_file: PathBuf::new(),
            admin_key_file: PathBuf::new(),
            timetable_secret_file: secret,
            timetable_school_id: "cbsh".into(),
            release: String::new(),
        };
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap();
        assert!(
            invoke(&client, &settings, "timetable-refresh")
                .await
                .is_err()
        );
        task.abort();
    }
}
