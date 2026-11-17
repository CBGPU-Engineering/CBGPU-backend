//! Explicit network-none disposable PG17 + fake GoTrue; never operating accounts or terminals.
#![cfg(unix)]
use axum::{
    Json, Router,
    body::Body,
    http::Request,
    routing::{get, post},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use cbgpu_portal_auth::{
    authority::{GoTrue, Oracle},
    core::{Core, Roster},
    crypto::{COOKIE, Crypto},
    store::Store,
    *,
};
use serde_json::{Value, json};
use sqlx::{
    PgPool,
    postgres::{PgConnectOptions, PgPoolOptions, PgSslMode},
};
use std::{
    os::unix::fs::PermissionsExt,
    process::Command,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tower::ServiceExt;
const KEY: &str = "fixture-session-secret-not-production-123456";
const UID: &str = "10000000-0000-4000-8000-000000000001";
const SID: &str = "20000000-0000-4000-8000-000000000001";
struct Db {
    container: String,
    socket: tempfile::TempDir,
}
impl Drop for Db {
    fn drop(&mut self) {
        let _ = Command::new("docker")
            .args(["rm", "-f", "-v", &self.container])
            .output();
    }
}
impl Db {
    async fn new() -> Self {
        let socket = tempfile::tempdir().unwrap();
        std::fs::set_permissions(socket.path(), std::fs::Permissions::from_mode(0o777)).unwrap();
        let container = format!(
            "cbgpu-portal-native-test-{}-{}",
            std::process::id(),
            rand::random::<u32>()
        );
        let mount = format!("{}:/var/run/postgresql", socket.path().display());
        let result = Command::new("docker")
            .args([
                "run",
                "-d",
                "--network",
                "none",
                "--memory",
                "384m",
                "--cpus",
                "1",
                "--name",
                &container,
                "-e",
                "POSTGRES_PASSWORD=fixture-only",
                "-v",
                &mount,
                "postgres:17-alpine",
                "postgres",
                "-c",
                "listen_addresses=",
            ])
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "Disposable PostgreSQL could not start"
        );
        let db = Self { container, socket };
        for _ in 0..100 {
            let logs = Command::new("docker")
                .args(["logs", &db.container])
                .output()
                .unwrap();
            let initialized = String::from_utf8_lossy(&logs.stdout)
                .contains("PostgreSQL init process complete")
                || String::from_utf8_lossy(&logs.stderr)
                    .contains("PostgreSQL init process complete");
            if initialized && db.pool("postgres", None).await.is_ok() {
                return db;
            }
            tokio::time::sleep(Duration::from_millis(100)).await
        }
        let logs = Command::new("docker")
            .args(["logs", &db.container])
            .output()
            .unwrap();
        panic!(
            "Disposable PostgreSQL startup timeout: {} {}",
            String::from_utf8_lossy(&logs.stdout),
            String::from_utf8_lossy(&logs.stderr)
        )
    }
    async fn pool(
        &self,
        database: &str,
        role: Option<&'static str>,
    ) -> std::result::Result<PgPool, sqlx::Error> {
        let options = PgConnectOptions::new()
            .host(self.socket.path().to_str().unwrap())
            .username("postgres")
            .password("fixture-only")
            .database(database)
            .ssl_mode(PgSslMode::Disable);
        PgPoolOptions::new()
            .max_connections(3)
            .acquire_timeout(Duration::from_secs(1))
            .after_connect(move |c, _| {
                Box::pin(async move {
                    if let Some(role) = role {
                        sqlx::query(&format!("SET ROLE {role}"))
                            .execute(&mut *c)
                            .await?;
                    }
                    Ok(())
                })
            })
            .connect_with(options)
            .await
    }
}
fn jwt() -> String {
    format!(
        "e30.{}.fixture_signature",
        URL_SAFE_NO_PAD.encode(
            serde_json::to_vec(&json!({"sub":UID,"session_id":SID,"exp":now_ms()/1000+3600}))
                .unwrap()
        )
    )
}
async fn request(
    app: axum::Router,
    path: &str,
    method: &str,
    cookie: &str,
    csrf: Option<&str>,
    body: &str,
) -> axum::response::Response {
    let mut r = Request::builder()
        .uri(path)
        .method(method)
        .header("host", "developers.cbgpu.com")
        .header("origin", "https://developers.cbgpu.com")
        .header("content-type", "application/json")
        .header("cookie", cookie);
    if let Some(csrf) = csrf {
        r = r.header("x-csrf-token", csrf)
    }
    app.oneshot(r.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap()
}
async fn json_response(response: axum::response::Response) -> Value {
    serde_json::from_slice(
        &axum::body::to_bytes(response.into_body(), 65536)
            .await
            .unwrap(),
    )
    .unwrap()
}
#[tokio::test]
#[ignore = "Explicit Docker postgres:17-alpine network-none fixture; no operating database"]
async fn real_pg_cookie_lifecycle_fresh_role_guard_and_readonly_import() {
    let db = Db::new().await;
    let owner = db.pool("postgres", None).await.unwrap();
    let owner_store = Store {
        pool: owner.clone(),
    };
    owner_store.bootstrap().await.unwrap();
    owner_store.bootstrap().await.unwrap();
    sqlx::query("CREATE DATABASE authority")
        .execute(&owner)
        .await
        .unwrap();
    sqlx::query("CREATE DATABASE imported")
        .execute(&owner)
        .await
        .unwrap();
    let authority_owner = db.pool("authority", None).await.unwrap();
    sqlx::raw_sql("CREATE ROLE anon NOLOGIN;CREATE ROLE authenticated NOLOGIN;CREATE ROLE service_role NOLOGIN;CREATE SCHEMA auth;CREATE TABLE auth.users(id UUID PRIMARY KEY,email TEXT,email_confirmed_at TIMESTAMPTZ,deleted_at TIMESTAMPTZ,banned_until TIMESTAMPTZ);CREATE TABLE auth.sessions(id UUID PRIMARY KEY,user_id UUID REFERENCES auth.users,not_after TIMESTAMPTZ);CREATE TABLE public.profiles(id UUID PRIMARY KEY,is_admin BOOL,is_approved BOOL,full_name TEXT,account_status TEXT);INSERT INTO auth.users VALUES('10000000-0000-4000-8000-000000000001','dev@example.invalid',now(),null,null);INSERT INTO auth.sessions VALUES('20000000-0000-4000-8000-000000000001','10000000-0000-4000-8000-000000000001',null);INSERT INTO public.profiles VALUES('10000000-0000-4000-8000-000000000001',true,false,'Developer','REVERIFICATION_REQUIRED');").execute(&authority_owner).await.unwrap();
    sqlx::raw_sql(include_str!(
        "../../../../supabase/migrations/20261117092500_portal_principal_authority.sql"
    ))
    .execute(&authority_owner)
    .await
    .unwrap();
    assert!(
        Store {
            pool: authority_owner.clone()
        }
        .bootstrap()
        .await
        .is_err(),
        "Auth database cannot become runtime store"
    );
    let api_calls = Arc::new(AtomicUsize::new(0));
    let calls = api_calls.clone();
    let token = jwt();
    let login_token = token.clone();
    let verify_token = token.clone();
    let fake=Router::new().route("/auth/v1/token",post(move|axum::extract::RawQuery(query):axum::extract::RawQuery,Json(body):Json<Value>|{let token=login_token.clone();let calls=calls.clone();async move{assert_eq!(query.as_deref(),Some("grant_type=password"),"must not rotate refresh family");assert_eq!(body["password"],"fixture-only");calls.fetch_add(1,Ordering::SeqCst);Json(json!({"user":{"id":UID},"access_token":token,"refresh_token":"fixture-refresh"}))}})).route("/auth/v1/user",get(move|headers:axum::http::HeaderMap|{let token=verify_token.clone();async move{assert_eq!(headers["authorization"],format!("Bearer {token}"));Json(json!({"id":UID}))}})).route("/rest/v1/rpc/cbgpu_self_session_status",post(||async{Json(json!({"valid":true,"approved":false}))}));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}/", listener.local_addr().unwrap());
    let auth_task = tokio::spawn(async move { axum::serve(listener, fake).await.unwrap() });
    let store = Store {
        pool: db
            .pool("postgres", Some("cbgpu_portal_session_store"))
            .await
            .unwrap(),
    };
    let oracle = Oracle::from_pool(
        db.pool("authority", Some("cbgpu_portal_auth_reader"))
            .await
            .unwrap(),
    );
    let (revoked, _) = tokio::sync::broadcast::channel(32);
    let core = Arc::new(Core {
        store: store.clone(),
        crypto: Crypto::new(KEY).unwrap(),
        roster: Roster::Fixed(vec![
            Member {
                id: UID.parse().unwrap(),
                email: "dev@example.invalid".into(),
            },
            Member {
                id: "10000000-0000-4000-8000-000000000002".parse().unwrap(),
                email: "other@example.invalid".into(),
            },
        ]),
        oracle,
        gotrue: GoTrue::new(&origin, "fixture-anon".into(), false).unwrap(),
        scoped: true,
        revoked,
    });
    let app = cbgpu_portal_auth::http::router(core.clone());
    let login = request(
        app.clone(),
        "/api/login",
        "POST",
        "",
        None,
        "{\"email\":\"dev@example.invalid\",\"password\":\"fixture-only\"}",
    )
    .await;
    assert_eq!(login.status(), 200);
    let cookie = login
        .headers()
        .get_all("set-cookie")
        .iter()
        .map(|v| v.to_str().unwrap())
        .find(|v| v.starts_with(&format!("{COOKIE}=")))
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string();
    let logged = json_response(login).await;
    assert_eq!(logged["user"]["id"], UID);
    assert_eq!(logged["csrf_token"].as_str().unwrap().len(), 64);
    assert!(
        logged.get("session").is_none(),
        "ordinary info must never expose GoTrue tokens"
    );
    assert_eq!(
        json_response(request(app.clone(), "/api/session", "GET", &cookie, None, "").await).await["authenticated"],
        true
    );
    assert_eq!(
        request(
            app.clone(),
            "/api/logout",
            "POST",
            &cookie,
            Some("wrong"),
            "{}"
        )
        .await
        .status(),
        403
    );
    let current = core.authenticate(&cookie).await.unwrap().unwrap();
    let expired = format!(
        "e30.{}.fixture_signature",
        URL_SAFE_NO_PAD.encode(
            serde_json::to_vec(&json!({"sub":UID,"session_id":SID,"exp":now_ms()/1000-60}))
                .unwrap()
        )
    );
    let expired_sealed = core
        .crypto
        .seal(&Tokens {
            access_token: expired,
            refresh_token: "fixture-refresh".into(),
        })
        .unwrap();
    sqlx::query("UPDATE portal_auth.sessions SET auth_tokens=$1 WHERE token_hash=$2")
        .bind(&expired_sealed)
        .bind(&current.row.token_hash)
        .execute(&owner)
        .await
        .unwrap();
    let continued = core.authenticate(&cookie).await.unwrap().unwrap();
    assert!(
        core.native_session(&continued).is_none(),
        "expired cached JWT must never leak as usable native session"
    );
    assert!(
        cbgpu_portal_auth::proxy_contract::bound_bearer(
            Some(&format!("Bearer {}", jwt())),
            &continued,
            core.native_session(&continued)
        )
        .is_ok(),
        "fresh matching caller bearer can continue without server refresh"
    );
    sqlx::query("UPDATE portal_auth.sessions SET auth_tokens=$1 WHERE token_hash=$2")
        .bind(&current.row.auth_tokens)
        .bind(&current.row.token_hash)
        .execute(&owner)
        .await
        .unwrap();
    sqlx::query(
        "ALTER FUNCTION cbgpu_portal_auth.principal(uuid,uuid) RENAME TO principal_outage_fixture",
    )
    .execute(&authority_owner)
    .await
    .unwrap();
    assert!(core.authenticate(&cookie).await.is_err());
    assert!(
        store.get(&current.row.token_hash).await.unwrap().is_some(),
        "authority outage must not erase a valid private session"
    );
    sqlx::query(
        "ALTER FUNCTION cbgpu_portal_auth.principal_outage_fixture(uuid,uuid) RENAME TO principal",
    )
    .execute(&authority_owner)
    .await
    .unwrap();
    assert!(core.authenticate(&cookie).await.unwrap().is_some());
    let (tx, mut rx) = tokio::sync::watch::channel(false);
    let hash = core
        .authenticate(&cookie)
        .await
        .unwrap()
        .unwrap()
        .row
        .token_hash;
    let guard = tokio::spawn(core.clone().monitor(hash, tx));
    tokio::time::sleep(Duration::from_millis(50)).await;
    sqlx::query("UPDATE public.profiles SET is_admin=false")
        .execute(&authority_owner)
        .await
        .unwrap();
    assert!(core.authenticate(&cookie).await.unwrap().is_none());
    tokio::time::timeout(Duration::from_secs(6), rx.changed())
        .await
        .unwrap()
        .unwrap();
    assert!(*rx.borrow());
    guard.await.unwrap();
    sqlx::query("UPDATE public.profiles SET is_admin=true")
        .execute(&authority_owner)
        .await
        .unwrap();
    assert!(
        core.authenticate(&cookie).await.unwrap().is_none(),
        "regrant cannot resurrect a definitively revoked portal cookie"
    );
    let relogin = request(
        app.clone(),
        "/api/login",
        "POST",
        "",
        None,
        "{\"email\":\"dev@example.invalid\",\"password\":\"fixture-only\"}",
    )
    .await;
    assert_eq!(relogin.status(), 200);
    let cookie = relogin
        .headers()
        .get_all("set-cookie")
        .iter()
        .map(|v| v.to_str().unwrap())
        .find(|v| v.starts_with(&format!("{COOKIE}=")))
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string();
    let relogged = json_response(relogin).await;
    let csrf = relogged["csrf_token"].as_str().unwrap();
    assert_eq!(
        request(
            app.clone(),
            "/api/logout",
            "POST",
            &cookie,
            Some(csrf),
            "{}"
        )
        .await
        .status(),
        200
    );
    assert!(core.authenticate(&cookie).await.unwrap().is_none());
    assert_eq!(
        api_calls.load(Ordering::SeqCst),
        2,
        "cookie requests never refresh GoTrue"
    );
    let imported_owner = db.pool("imported", None).await.unwrap();
    Store {
        pool: imported_owner.clone(),
    }
    .bootstrap()
    .await
    .unwrap();
    let imported = Store {
        pool: db
            .pool("imported", Some("cbgpu_portal_session_store"))
            .await
            .unwrap(),
    };
    let file = tempfile::NamedTempFile::new().unwrap();
    let sqlite = rusqlite::Connection::open(file.path()).unwrap();
    sqlite.execute_batch("CREATE TABLE sessions(token_hash TEXT PRIMARY KEY,csrf TEXT,expires INTEGER,user_id TEXT,email TEXT,auth_version TEXT,auth_session_id TEXT,auth_tokens TEXT);CREATE TABLE failures(key TEXT PRIMARY KEY,count INTEGER,expires INTEGER);CREATE TABLE audit_events(sql_text TEXT);INSERT INTO audit_events VALUES('SELECT fixture_only');").unwrap();
    let hash = core.crypto.hash(&"a".repeat(64));
    let sealed = core
        .crypto
        .seal(&Tokens {
            access_token: token,
            refresh_token: "fixture-refresh".into(),
        })
        .unwrap();
    sqlite
        .execute(
            "INSERT INTO sessions VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
            rusqlite::params![
                hash,
                "b".repeat(64),
                now_ms() + 3_600_000,
                UID,
                "dev@example.invalid",
                core.crypto.version(UID.parse().unwrap()),
                SID,
                sealed
            ],
        )
        .unwrap();
    drop(sqlite);
    let before = std::fs::read(file.path()).unwrap();
    assert!(
        importer::import_read_only(
            &imported,
            file.path(),
            &Crypto::new("different-secret-fixture-123456789").unwrap()
        )
        .await
        .is_err()
    );
    let untouched: i64 = sqlx::query_scalar("SELECT count(*) FROM portal_auth.sessions")
        .fetch_one(&imported.pool)
        .await
        .unwrap();
    assert_eq!(untouched, 0, "wrong snapshot key must not partially import");
    let report = importer::import_read_only(&imported, file.path(), &core.crypto)
        .await
        .unwrap();
    assert_eq!(report.sessions, 1);
    assert!(
        report
            .untouched_source_tables
            .contains(&"audit_events".into())
    );
    assert_eq!(before, std::fs::read(file.path()).unwrap());
    let row = imported.get(&hash).await.unwrap().unwrap();
    assert_eq!(row.auth_tokens, sealed);
    assert_eq!(row.csrf, "b".repeat(64));
    imported.delete(&hash).await.unwrap();
    assert!(
        importer::import_read_only(&imported, file.path(), &core.crypto)
            .await
            .unwrap()
            .already_imported
    );
    assert!(
        imported.get(&hash).await.unwrap().is_none(),
        "repeated import must not resurrect logged-out sessions"
    );
    auth_task.abort();
}
