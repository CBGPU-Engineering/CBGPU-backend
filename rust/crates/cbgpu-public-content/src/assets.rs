//! Opaque contest banner alias: fixed internal origin, private resolver, no redirects.
//! Dedicated NOINHERIT login membership is required; raw auth/public/storage
//! table or column privileges fail startup. Catalog OIDs avoid granting auth
//! schema access merely to inspect privileges. Each connection SET ROLEs to the
//! reviewed non-login resolver and enforces read-only transactions.
use axum::{
    body::Body,
    http::{StatusCode, header},
    response::Response,
};
use serde_json::Value;
use sqlx::{PgPool, postgres::PgPoolOptions};
use std::{collections::HashSet, sync::Arc, time::Duration};
use tokio::sync::Semaphore;
use url::Url;
use uuid::Uuid;
pub const LIMIT: usize = 10 * 1024 * 1024;
#[derive(Clone)]
pub struct Assets {
    pool: PgPool,
    origin: Url,
    client: reqwest::Client,
    downloads: Arc<Semaphore>,
}
impl Assets {
    pub async fn connect(
        database: &str,
        storage: &str,
        network_mode: Option<&str>,
    ) -> Result<Self, String> {
        let origin = storage_origin(storage, network_mode)?;
        let pool = PgPoolOptions::new()
            .max_connections(4)
            .acquire_timeout(Duration::from_secs(3))
            .after_connect(|connection, _| {
                Box::pin(async move {
                    let safe:bool=sqlx::query_scalar("SELECT NOT login.rolsuper AND NOT login.rolbypassrls AND NOT login.rolcreatedb AND NOT login.rolcreaterole AND NOT login.rolreplication AND pg_has_role(session_user,'cbgpu_public_asset_reader','member') AND NOT pg_has_role(session_user,'authenticated','member') AND NOT pg_has_role(session_user,'service_role','member') AND NOT reader.rolsuper AND NOT reader.rolbypassrls AND NOT reader.rolcanlogin AND NOT reader.rolinherit AND NOT has_schema_privilege(session_user,'cbgpu_schools','CREATE') AND has_function_privilege('cbgpu_public_asset_reader',(SELECT p.oid FROM pg_proc p JOIN pg_namespace pn ON pn.oid=p.pronamespace WHERE pn.nspname='cbgpu_schools' AND p.proname='public_contest_banner' AND p.proargtypes='2950'::oidvector),'EXECUTE') AND NOT EXISTS(SELECT 1 FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname IN('auth','public','storage','cbgpu_schools') AND c.relkind IN('r','p','v','m','f') AND(has_table_privilege(session_user,c.oid,'SELECT,INSERT,UPDATE,DELETE,TRUNCATE,REFERENCES,TRIGGER')OR has_any_column_privilege(session_user,c.oid,'SELECT,INSERT,UPDATE,REFERENCES')OR has_table_privilege('cbgpu_public_asset_reader',c.oid,'SELECT,INSERT,UPDATE,DELETE,TRUNCATE,REFERENCES,TRIGGER')OR has_any_column_privilege('cbgpu_public_asset_reader',c.oid,'SELECT,INSERT,UPDATE,REFERENCES'))) FROM pg_roles login CROSS JOIN pg_roles reader WHERE login.rolname=session_user AND reader.rolname='cbgpu_public_asset_reader'").fetch_one(&mut *connection).await?;
                    if !safe{return Err(sqlx::Error::Protocol("Public asset login must be a dedicated EXECUTE-only reader".into()));}
                    sqlx::query("SET ROLE cbgpu_public_asset_reader")
                        .execute(&mut *connection)
                        .await?;
                    sqlx::query("SET default_transaction_read_only=on").execute(&mut *connection).await?;
                    sqlx::query("SET statement_timeout='3000ms'")
                        .execute(connection)
                        .await?;
                    Ok(())
                })
            })
            .connect(database)
            .await
            .map_err(|_| "Public asset resolver unavailable".to_string())?;
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(12))
            .build()
            .map_err(|_| "Public asset client unavailable".to_string())?;
        Ok(Self {
            pool,
            origin,
            client,
            downloads: Arc::new(Semaphore::new(8)),
        })
    }
    async fn resolve(&self, id: Uuid) -> Result<Option<Descriptor>, ()> {
        let value: Option<Value> =
            sqlx::query_scalar("SELECT cbgpu_schools.public_contest_banner($1)")
                .bind(id)
                .fetch_one(&self.pool)
                .await
                .map_err(|_| ())?;
        value.as_ref().map(descriptor).transpose()
    }
    pub async fn available(&self, ids: &[Uuid]) -> HashSet<Uuid> {
        if ids.len() > 100 {
            return HashSet::new();
        }
        let query = sqlx::query_as::<_, (Uuid, Option<Value>)>(
            "SELECT id,cbgpu_schools.public_contest_banner(id) FROM unnest($1::uuid[]) AS id",
        )
        .bind(ids)
        .fetch_all(&self.pool);
        match tokio::time::timeout(Duration::from_secs(1), query).await {
            Ok(Ok(rows)) => rows
                .into_iter()
                .filter_map(|(id, value)| value.filter(|v| descriptor(v).is_ok()).map(|_| id))
                .collect(),
            _ => HashSet::new(),
        }
    }
    pub async fn get(&self, id: Uuid) -> Response {
        let permit = match tokio::time::timeout(
            Duration::from_secs(1),
            self.downloads.clone().acquire_owned(),
        )
        .await
        {
            Ok(Ok(p)) => p,
            _ => return failure(StatusCode::TOO_MANY_REQUESTS),
        };
        let location = match self.resolve(id).await {
            Ok(Some(v)) => v,
            Ok(None) => return failure(StatusCode::NOT_FOUND),
            Err(_) => return failure(StatusCode::SERVICE_UNAVAILABLE),
        };
        let bytes = match self.read(&location).await {
            Ok(v) => v,
            Err(_) => return failure(StatusCode::NOT_FOUND),
        };
        // Recheck after a potentially slow Storage read: revoked visibility must not
        // race a previously authorized fetch. No cached descriptor is authoritative.
        if !matches!(self.resolve(id).await,Ok(Some(current))if current==location) {
            return failure(StatusCode::NOT_FOUND);
        }
        Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, &location.mime)
            .header(header::CONTENT_LENGTH, bytes.len().to_string())
            .header(header::CACHE_CONTROL, "private, no-store, max-age=0")
            .header(header::PRAGMA, "no-cache")
            .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff")
            .header(
                header::CONTENT_SECURITY_POLICY,
                "sandbox; default-src 'none'; frame-ancestors 'none'",
            )
            .header(header::REFERRER_POLICY, "no-referrer")
            .body(Body::from_stream(futures_util::stream::unfold(
                (Some(bytes), permit),
                |(bytes, permit)| async move {
                    bytes.map(|bytes| (Ok::<_, std::io::Error>(bytes), (None, permit)))
                },
            )))
            .unwrap()
    }
    async fn read(&self, location: &Descriptor) -> Result<Vec<u8>, ()> {
        read(&self.client, &self.origin, location).await
    }
}
#[derive(Debug, PartialEq, Eq)]
pub struct Descriptor {
    key: String,
    mime: String,
    size: usize,
}
fn descriptor(v: &Value) -> Result<Descriptor, ()> {
    if v["bucket"] != "contest-presentation" {
        return Err(());
    }
    let key = v["object_key"].as_str().ok_or(())?;
    let (owner, file) = key.split_once('/').ok_or(())?;
    let (object, extension) = file.rsplit_once('.').ok_or(())?;
    if Uuid::parse_str(owner).is_err()
        || Uuid::parse_str(object).is_err()
        || file.contains('/')
        || !matches!(extension, "png" | "jpg" | "webp")
    {
        return Err(());
    }
    // UUID parsing alone accepts noncanonical variants; only canonical path segments.
    if Uuid::parse_str(owner).unwrap().to_string() != owner
        || Uuid::parse_str(object).unwrap().to_string() != object
    {
        return Err(());
    }
    let mime = v["mime"].as_str().ok_or(())?;
    if !matches!(
        (extension, mime),
        ("png", "image/png") | ("jpg", "image/jpeg") | ("webp", "image/webp")
    ) {
        return Err(());
    }
    let size = v["size"]
        .as_u64()
        .and_then(|v| usize::try_from(v).ok())
        .filter(|v| (1..=LIMIT).contains(v))
        .ok_or(())?;
    Ok(Descriptor {
        key: key.into(),
        mime: mime.into(),
        size,
    })
}
pub fn alias(id: Uuid) -> String {
    format!("/api/public-assets/contests/{id}/banner")
}
pub fn storage_origin(value: &str, mode: Option<&str>) -> Result<Url, String> {
    let invalid = || {
        "Public Storage requires a fixed loopback or explicit private-container origin".to_string()
    };
    let u = Url::parse(value).map_err(|_| invalid())?;
    if !matches!(u.scheme(), "http" | "https")
        || !u.username().is_empty()
        || u.password().is_some()
        || u.query().is_some()
        || u.fragment().is_some()
        || u.path() != "/"
    {
        return Err(invalid());
    }
    let host = u.host_str().ok_or_else(invalid)?;
    let local = matches!(host, "localhost" | "127.0.0.1" | "[::1]");
    let dns = host.split('.').all(|s| {
        !s.is_empty()
            && s.bytes()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
            && !s.starts_with('-')
            && !s.ends_with('-')
    }) && (!host.contains('.')
        || host.ends_with(".internal")
        || host.ends_with(".svc.cluster.local"));
    if !local && !(mode == Some("private-container") && dns) {
        return Err(invalid());
    }
    Ok(u)
}
fn magic(mime: &str, b: &[u8]) -> bool {
    match mime {
        "image/png" => {
            b.len() >= 24 && b.starts_with(b"\x89PNG\r\n\x1a\n") && &b[12..16] == b"IHDR"
        }
        "image/jpeg" => b.len() >= 4 && b.starts_with(&[255, 216]) && b.ends_with(&[255, 217]),
        "image/webp" => {
            b.len() >= 16
                && b.starts_with(b"RIFF")
                && &b[8..12] == b"WEBP"
                && matches!(&b[12..16], b"VP8 " | b"VP8L" | b"VP8X")
        }
        _ => false,
    }
}
async fn read(client: &reqwest::Client, origin: &Url, d: &Descriptor) -> Result<Vec<u8>, ()> {
    let mut url = origin.clone();
    {
        let mut path = url.path_segments_mut().map_err(|_| ())?;
        path.clear()
            .extend(["storage", "v1", "object", "public", "contest-presentation"]);
        path.extend(d.key.split('/'));
    }
    let mut response = client
        .get(url)
        .header(header::ACCEPT, "image/png,image/jpeg,image/webp")
        .send()
        .await
        .map_err(|_| ())?;
    if response.status() != reqwest::StatusCode::OK
        || response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(|v| v.split(';').next().unwrap_or("").trim())
            != Some(d.mime.as_str())
        || response.headers().contains_key(header::CONTENT_ENCODING)
        || response
            .content_length()
            .is_some_and(|size| size != d.size as u64 || size > LIMIT as u64)
    {
        return Err(());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| ())? {
        if bytes.len().saturating_add(chunk.len()) > d.size
            || bytes.len().saturating_add(chunk.len()) > LIMIT
        {
            return Err(());
        }
        bytes.extend_from_slice(&chunk)
    }
    if bytes.len() != d.size || !magic(&d.mime, &bytes) {
        return Err(());
    }
    Ok(bytes)
}
pub fn failure(status: StatusCode) -> Response {
    Response::builder()
        .status(status)
        .header(header::CACHE_CONTROL, "private, no-store, max-age=0")
        .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff")
        .header(
            header::CONTENT_SECURITY_POLICY,
            "sandbox; default-src 'none'; frame-ancestors 'none'",
        )
        .body(Body::empty())
        .unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Router, routing::get};
    fn png() -> Vec<u8> {
        vec![
            137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1,
            8, 4, 0, 0, 0, 181, 28, 12, 2, 0, 0, 0, 11, 73, 68, 65, 84, 120, 218, 99, 100, 248, 15,
            0, 1, 5, 1, 1, 39, 24, 227, 102, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
        ]
    }
    fn item() -> Descriptor {
        Descriptor {
            key: "10000000-0000-4000-8000-000000000001/20000000-0000-4000-8000-000000000001.png"
                .into(),
            mime: "image/png".into(),
            size: png().len(),
        }
    }
    #[test]
    fn fixed_operator_origin_rejects_public_hosts_and_user_url_parts() {
        for origin in [
            "https://example.com/",
            "http://127.0.0.1/?url=evil",
            "http://user:secret@127.0.0.1/",
            "http://127.0.0.1/path",
            "file:///etc/passwd",
            "http://169.254.169.254/",
        ] {
            assert!(storage_origin(origin, None).is_err());
        }
        assert!(storage_origin("http://gateway:8000/", Some("private-container")).is_ok());
        assert!(storage_origin("http://gateway:8000/", None).is_err());
        assert!(storage_origin("https://attacker.com/", Some("private-container")).is_err());
    }
    #[test]
    fn descriptor_is_only_attributed_canonical_image_key() {
        let good = json_descriptor();
        assert!(descriptor(&good).is_ok());
        for field in [
            serde_json::json!({"bucket":"private-bucket"}),
            serde_json::json!({"mime":"image/svg+xml"}),
            serde_json::json!({"size":LIMIT+1}),
            serde_json::json!({"object_key":"../secret.png"}),
        ] {
            let mut v = good.clone();
            for (k, vv) in field.as_object().unwrap() {
                v[k] = vv.clone()
            }
            assert!(descriptor(&v).is_err());
        }
        let id = Uuid::parse_str("30000000-0000-4000-8000-000000000001").unwrap();
        assert_eq!(
            alias(id),
            format!("/api/public-assets/contests/{id}/banner")
        );
        assert!(!alias(id).contains(&item().key));
    }
    fn json_descriptor() -> Value {
        serde_json::json!({"bucket":"contest-presentation","object_key":item().key,"mime":"image/png","size":png().len()})
    }
    #[tokio::test]
    async fn storage_bytes_preserved_and_no_browser_credentials_forwarded() {
        let bytes = png();
        let original = bytes.clone();
        let app = Router::new().route(
            "/storage/v1/object/public/contest-presentation/{owner}/{file}",
            get(move |headers: axum::http::HeaderMap| {
                let bytes = bytes.clone();
                async move {
                    assert!(headers.get(header::COOKIE).is_none());
                    assert!(headers.get(header::AUTHORIZATION).is_none());
                    assert!(headers.get("apikey").is_none());
                    Response::builder()
                        .header(header::CONTENT_TYPE, "image/png")
                        .body(Body::from(bytes))
                        .unwrap()
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = Url::parse(&format!("http://{}/", listener.local_addr().unwrap())).unwrap();
        let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap();
        assert_eq!(read(&client, &origin, &item()).await.unwrap(), original);
        task.abort();
    }
    #[tokio::test]
    async fn html_svg_oversize_and_redirects_never_become_public_images() {
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };
        let redirected = Arc::new(AtomicUsize::new(0));
        let count = redirected.clone();
        for mode in [
            "html", "svg", "redirect", "oversize", "mismatch", "encoding",
        ] {
            let app = Router::new()
                .route(
                    "/storage/v1/object/public/contest-presentation/{owner}/{file}",
                    get(move || async move {
                        let mut r = Response::builder();
                        let mut b = png();
                        match mode {
                            "html" => {
                                b = b"<html><script>steal()</script></html>".to_vec();
                                r = r.header(header::CONTENT_TYPE, "image/png")
                            }
                            "svg" => r = r.header(header::CONTENT_TYPE, "image/svg+xml"),
                            "redirect" => r = r.status(302).header(header::LOCATION, "/sensitive"),
                            "oversize" => {
                                r = r.header(header::CONTENT_TYPE, "image/png");
                                b = vec![0; LIMIT + 1]
                            }
                            "encoding" => {
                                r = r
                                    .header(header::CONTENT_TYPE, "image/png")
                                    .header(header::CONTENT_ENCODING, "gzip")
                            }
                            _ => r = r.header(header::CONTENT_TYPE, "image/jpeg"),
                        }
                        r.body(Body::from(b)).unwrap()
                    }),
                )
                .route(
                    "/sensitive",
                    get({
                        let c = count.clone();
                        move || {
                            c.fetch_add(1, Ordering::SeqCst);
                            async { "not requested" }
                        }
                    }),
                );
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let origin =
                Url::parse(&format!("http://{}/", listener.local_addr().unwrap())).unwrap();
            let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
            let client = reqwest::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .unwrap();
            assert!(
                read(&client, &origin, &item()).await.is_err(),
                "mode {mode}"
            );
            task.abort();
        }
        assert_eq!(redirected.load(Ordering::SeqCst), 0);
    }
    #[test]
    fn response_errors_never_include_descriptor_or_cacheable_body() {
        let r = failure(StatusCode::NOT_FOUND);
        assert_eq!(
            r.headers()[header::CACHE_CONTROL],
            "private, no-store, max-age=0"
        );
        assert!(r.headers().get(header::LOCATION).is_none());
        assert_eq!(r.headers()[header::X_CONTENT_TYPE_OPTIONS], "nosniff");
    }
    #[tokio::test]
    #[ignore = "explicit private synthetic PostgreSQL HTTP stage only"]
    async fn actual_stage_reader_rejects_raw_columns_and_authority_drift() {
        let file = std::env::var("CBGPU_PUBLIC_ASSET_STAGE_DATABASE_URL_FILE")
            .expect("Private stage DSN file required");
        let meta = std::fs::symlink_metadata(&file).unwrap();
        assert!(meta.is_file());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(meta.permissions().mode() & 0o077, 0)
        }
        let dsn = std::fs::read_to_string(file).unwrap();
        let url = Url::parse(dsn.trim()).unwrap();
        assert_eq!(url.host_str(), Some("127.0.0.1"));
        assert_eq!(url.port(), Some(55485));
        assert_eq!(url.username(), "cbgpu_stage_public_asset");
        let store = Assets::connect(dsn.trim(), "http://127.0.0.1:9", None)
            .await
            .unwrap();
        let role: String = sqlx::query_scalar("select current_user::text")
            .fetch_one(&store.pool)
            .await
            .unwrap();
        assert_eq!(role, "cbgpu_public_asset_reader");
        for query in [
            "select id from auth.users limit 1",
            "select full_name from public.profiles limit 1",
            "select name from storage.objects limit 1",
            "set role authenticated",
        ] {
            assert!(
                sqlx::query(query).execute(&store.pool).await.is_err(),
                "raw authority must remain denied"
            )
        }
        assert!(store.resolve(Uuid::nil()).await.unwrap().is_none());
        let name = std::env::var("CBGPU_PUBLIC_ASSET_STAGE_CONTAINER").unwrap();
        assert!(
            name.strip_prefix("cbgpu-school-http-stage-")
                .is_some_and(|x| !x.is_empty() && x.bytes().all(|b| b.is_ascii_digit()))
        );
        fn sql(name: &str, statement: &str) {
            use std::io::Write;
            use std::process::{Command, Stdio};
            let mut c = Command::new("docker")
                .args([
                    "exec",
                    "-i",
                    name,
                    "psql",
                    "-h",
                    "/tmp",
                    "-U",
                    "cbgpu_clone_bootstrap",
                    "-d",
                    "postgres",
                    "-v",
                    "ON_ERROR_STOP=1",
                    "-qAt",
                ])
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap();
            c.stdin
                .take()
                .unwrap()
                .write_all(statement.as_bytes())
                .unwrap();
            assert!(c.wait().unwrap().success());
        }
        struct Reset(String);
        impl Drop for Reset {
            fn drop(&mut self) {
                sql(
                    &self.0,
                    "revoke select(full_name)on public.profiles from cbgpu_stage_public_asset;",
                )
            }
        }
        sql(
            &name,
            "grant select(full_name)on public.profiles to cbgpu_stage_public_asset;",
        );
        let reset = Reset(name);
        assert!(
            Assets::connect(dsn.trim(), "http://127.0.0.1:9", None)
                .await
                .is_err()
        );
        drop(reset);
        assert!(
            Assets::connect(dsn.trim(), "http://127.0.0.1:9", None)
                .await
                .is_ok()
        );
    }
}
