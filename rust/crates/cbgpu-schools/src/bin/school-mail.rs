//! Dedicated EXECUTE-only outbox consumer; never runs inside a browser/API request.
use serde_json::{Value, json};
use sqlx::postgres::PgPoolOptions;
use uuid::Uuid;

fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}
fn payload(job: &Value, from: &str) -> Result<Value, &'static str> {
    if job["recipient"] != "info@cbgpu.com" || job["template"] != "school-feature-request" {
        return Err("INVALID_MAIL_INTENT");
    }
    let field = |key: &str, max| {
        job[key]
            .as_str()
            .filter(|s| !s.is_empty() && s.len() <= max)
            .ok_or("INVALID_MAIL_INTENT")
    };
    let school = field("school_name", 600)?;
    let title = field("title", 600)?;
    let description = field("description", 16000)?;
    let id = field("school_id", 100)?;
    if !id
        .bytes()
        .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
    {
        return Err("INVALID_MAIL_INTENT");
    }
    let request = field("request_id", 36)?;
    Uuid::parse_str(request).map_err(|_| "INVALID_MAIL_INTENT")?;
    let subject = format!("[CBGPU 학교 기능 요청] {} — {}", school, title)
        .chars()
        .filter(|c| !c.is_control())
        .take(250)
        .collect::<String>();
    let url = "https://admin.cbgpu.com/admin/feature-requests";
    let html = format!(
        r#"<!doctype html><html lang="ko"><meta charset="utf-8"><body style="margin:0;background:#f4f6f8;font-family:Arial,sans-serif;color:#191f28"><table role="presentation" style="width:100%;padding:28px"><tr><td><div style="max-width:600px;margin:auto;background:white;padding:32px;border-radius:16px"><a href="https://cbgpu.com"><img src="https://cbgpu.com/brand/v1/symbol_black.png" alt="CBGPU" width="40" height="40"></a><h1 style="font-size:24px">학교 전용 기능 요청</h1><p>{}</p><h2 style="font-size:18px">{}</h2><p style="line-height:1.8">{}</p><p style="font-size:12px;color:#667085">요청 번호: {}</p><a href="{}" style="display:inline-block;background:#17634d;color:white;text-decoration:none;border-radius:8px;padding:14px 20px">학교 관리 화면 열기</a><p style="font-size:13px;color:#667085">이 메일은 요청 접수 알림입니다. 기능 승인 또는 개발 완료를 의미하지 않습니다.</p><p style="font-size:12px">CBGPU · info@cbgpu.com</p></div></td></tr></table></body></html>"#,
        escape(school),
        escape(title),
        escape(description).replace('\n', "<br>"),
        escape(request),
        escape(url)
    );
    Ok(
        json!({"from":from,"to":["info@cbgpu.com"],"subject":subject,"html":html,"text":format!("학교 전용 기능 요청\n학교: {school}\n제목: {title}\n\n{description}\n\n요청 번호: {request}\n{url}\n요청 접수는 승인이나 구현 완료를 의미하지 않습니다.")}),
    )
}

fn credential(name: &str) -> Result<String, Box<dyn std::error::Error>> {
    let file = std::env::var(format!("{name}_FILE")).ok();
    let value = std::env::var(name).ok();
    match (file, value) {
        (Some(path), None) => {
            let meta = std::fs::symlink_metadata(&path)?;
            if !meta.is_file() {
                return Err("Credential must be a regular private file".into());
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if meta.permissions().mode() & 0o077 != 0 {
                    return Err("Credential file must be private".into());
                }
            }
            let value = std::fs::read_to_string(path)?.trim().to_owned();
            if value.is_empty() || value.len() > 8192 {
                return Err("Invalid credential file".into());
            }
            Ok(value)
        }
        (None, Some(value)) if !value.trim().is_empty() => Ok(value),
        _ => Err("Choose exactly one private credential source".into()),
    }
}
async fn deliver(
    client: &reqwest::Client,
    job: &Value,
    from: &str,
    key: &str,
    url: &str,
) -> (bool, &'static str) {
    let body = match payload(job, from) {
        Ok(body) => body,
        Err(code) => return (false, code),
    };
    let Some(id) = job["id"].as_str().and_then(|s| Uuid::parse_str(s).ok()) else {
        return (false, "INVALID_MAIL_INTENT");
    };
    let response = match client
        .post(url)
        .bearer_auth(key)
        .header("Idempotency-Key", format!("cbgpu-school-mail-{id}"))
        .json(&body)
        .send()
        .await
    {
        Ok(v) => v,
        Err(_) => return (false, "MAIL_PROVIDER_UNAVAILABLE"),
    };
    let status = response.status();
    if status.is_success() {
        if let Ok(body) = response.json::<Value>().await
            && body["id"].as_str().is_some_and(|s| !s.is_empty())
        {
            return (true, "MAIL_PROVIDER_ACCEPTED");
        }
        return (false, "MAIL_PROVIDER_UNAVAILABLE");
    }
    (
        false,
        if status.as_u16() == 429 {
            "MAIL_PROVIDER_RATE_LIMIT"
        } else if status.is_client_error() {
            "MAIL_PROVIDER_REJECTED"
        } else {
            "MAIL_PROVIDER_UNAVAILABLE"
        },
    )
}
async fn reviewed_pool(database_url: &str) -> Result<sqlx::PgPool, Box<dyn std::error::Error>> {
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .acquire_timeout(std::time::Duration::from_secs(10))
        .connect(database_url)
        .await?;
    let safe:bool=sqlx::query_scalar("SELECT NOT r.rolsuper AND NOT r.rolbypassrls AND pg_has_role(current_user,'cbgpu_school_mail_worker','member') AND NOT pg_has_role(current_user,'authenticated','member') AND NOT pg_has_role(current_user,'service_role','member') AND NOT has_schema_privilege(current_user,'cbgpu_schools','CREATE') AND NOT EXISTS(SELECT 1 FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace WHERE(n.nspname,c.relname)IN(('auth','users'),('auth','sessions'),('public','profiles'),('cbgpu_schools','mail_outbox'),('cbgpu_schools','memberships'),('cbgpu_schools','feature_requests')) AND(has_table_privilege(current_user,c.oid,'SELECT,INSERT,UPDATE,DELETE,TRUNCATE,REFERENCES,TRIGGER') OR has_any_column_privilege(current_user,c.oid,'SELECT,INSERT,UPDATE,REFERENCES'))) AND has_function_privilege(current_user,'cbgpu_schools.claim_mail(uuid)','EXECUTE') AND has_function_privilege(current_user,'cbgpu_schools.ack_mail(uuid,uuid,boolean,text)','EXECUTE') FROM pg_roles r WHERE r.rolname=current_user").fetch_one(&pool).await?;
    if !safe {
        return Err("Mail runtime must use reviewed EXECUTE-only credentials".into());
    }
    Ok(pool)
}
async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let arguments = std::env::args().skip(1).collect::<Vec<_>>();
    if arguments == ["--help"] {
        println!(
            "school-mail [--check|--once]: private reviewed outbox worker; --check never claims or contacts the provider"
        );
        return Ok(());
    }
    if !arguments.is_empty() && arguments != ["--check"] && arguments != ["--once"] {
        return Err("Unsupported school mail option".into());
    }
    if std::env::var("CBGPU_SCHOOL_MAIL_ENABLED").as_deref() != Ok("1") {
        return Err("School mail worker is disabled".into());
    }
    let api_key = credential("RESEND_API_KEY")?;
    let from = std::env::var("CBGPU_EMAIL_FROM").unwrap_or("CBGPU <info@cbgpu.com>".into());
    if api_key.trim().is_empty() || from.contains(['\r', '\n']) {
        return Err("Invalid private mail configuration".into());
    }
    let pool = reviewed_pool(&credential("CBGPU_SCHOOL_MAIL_DATABASE_URL")?).await?;
    if std::env::args().any(|a| a == "--check") {
        println!(
            "School mail private configuration and EXECUTE-only database authority validated; no claim or provider request."
        );
        return Ok(());
    }
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(20))
        .build()?;
    let once = std::env::args().any(|a| a == "--once");
    let (stop_tx, mut stop_rx) = tokio::sync::watch::channel(false);
    tokio::spawn(async move {
        shutdown().await;
        let _ = stop_tx.send(true);
    });
    loop {
        if *stop_rx.borrow() {
            break;
        }
        let lease = Uuid::new_v4();
        let job: Option<Value> = sqlx::query_scalar("SELECT cbgpu_schools.claim_mail($1)")
            .bind(lease)
            .fetch_one(&pool)
            .await?;
        if let Some(job) = job {
            let id = job["id"]
                .as_str()
                .and_then(|s| Uuid::parse_str(s).ok())
                .ok_or("Invalid claimed job")?;
            let (success, code) = deliver(
                &client,
                &job,
                &from,
                &api_key,
                "https://api.resend.com/emails",
            )
            .await;
            let acknowledged: bool =
                sqlx::query_scalar("SELECT cbgpu_schools.ack_mail($1,$2,$3,$4)")
                    .bind(id)
                    .bind(lease)
                    .bind(success)
                    .bind(code)
                    .fetch_one(&pool)
                    .await?;
            // Never log school names, message contents, recipient credentials or provider responses.
            println!("school-mail status={code} lease_acknowledged={acknowledged}");
        }
        if once {
            break;
        }
        tokio::select! {_=stop_rx.changed()=>break,_=tokio::time::sleep(std::time::Duration::from_secs(5))=>{}}
    }
    Ok(())
}

async fn shutdown() {
    #[cfg(unix)]
    {
        if let Ok(mut term) =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            tokio::select! {_=tokio::signal::ctrl_c()=>{},_=term.recv()=>{}}
            return;
        }
    }
    let _ = tokio::signal::ctrl_c().await;
}

#[tokio::main]
async fn main() {
    if run().await.is_err() {
        eprintln!(
            "School mail worker failed; check private configuration, database permissions and queue health."
        );
        std::process::exit(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn job() -> Value {
        json!({"id":"fb900000-0000-4000-8000-000000000001","recipient":"info@cbgpu.com","template":"school-feature-request","school_name":"학교 <script>","school_id":"school-example","title":"요청\r\nInjected: header","description":"<img src=x onerror=alert(1)>\n둘째 줄","request_id":"218bf41f-c40c-4497-a170-52c642880d8b"})
    }
    #[test]
    fn html_is_escaped_and_subject_cannot_inject_headers() {
        let p = payload(&job(), "CBGPU <info@cbgpu.com>").unwrap();
        assert!(!p["subject"].as_str().unwrap().contains(['\r', '\n']));
        let h = p["html"].as_str().unwrap();
        assert!(!h.contains("<script>"));
        assert!(h.contains("&lt;img src=x onerror=alert(1)&gt;<br>"));
        assert_eq!(p["to"], json!(["info@cbgpu.com"]));
    }
    #[test]
    fn queue_cannot_redirect_email_or_choose_template() {
        let mut j = job();
        j["recipient"] = json!("other@example.invalid");
        assert!(payload(&j, "info@cbgpu.com").is_err());
        j = job();
        j["template"] = json!("recovery");
        assert!(payload(&j, "info@cbgpu.com").is_err());
    }
    #[test]
    fn school_cannot_inject_a_destination_path() {
        let mut j = job();
        j["school_id"] = json!("school/../../auth");
        assert!(payload(&j, "info@cbgpu.com").is_err());
    }
    #[tokio::test]
    async fn provider_stub_acceptance_and_retry_keep_same_idempotency_intent() {
        use axum::{Json, Router, extract::State, http::HeaderMap, routing::post};
        use std::sync::{Arc, Mutex};
        let seen = Arc::new(Mutex::new(Vec::<String>::new()));
        let calls = seen.clone();
        let app = Router::new()
            .route(
                "/emails",
                post(
                    |State(calls): State<Arc<Mutex<Vec<String>>>>,
                     headers: HeaderMap,
                     Json(value): Json<Value>| async move {
                        assert_eq!(
                            headers["authorization"],
                            "Bearer synthetic-not-a-provider-key"
                        );
                        assert_eq!(value["to"], json!(["info@cbgpu.com"]));
                        calls
                            .lock()
                            .unwrap()
                            .push(headers["idempotency-key"].to_str().unwrap().into());
                        Json(json!({"id":"synthetic-accepted-not-delivered"}))
                    },
                ),
            )
            .with_state(calls);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap();
        let url = format!("http://{addr}/emails");
        for _ in 0..2 {
            assert_eq!(
                deliver(
                    &client,
                    &job(),
                    "CBGPU <info@cbgpu.com>",
                    "synthetic-not-a-provider-key",
                    &url
                )
                .await,
                (true, "MAIL_PROVIDER_ACCEPTED")
            );
        }
        let calls = seen.lock().unwrap();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0], calls[1]);
        assert_eq!(
            calls[0],
            "cbgpu-school-mail-fb900000-0000-4000-8000-000000000001"
        );
        task.abort();
    }
    #[tokio::test]
    #[ignore = "explicit synthetic loopback school HTTP stage only"]
    async fn real_stage_exec_only_claim_provider_stub_and_ack() {
        let path = std::env::var("CBGPU_SCHOOL_MAIL_STAGE_CREDENTIALS_FILE")
            .expect("Provide private synthetic stage credentials path");
        let meta = std::fs::symlink_metadata(&path).unwrap();
        assert!(meta.is_file());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(meta.permissions().mode() & 0o077, 0)
        }
        let config: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        let dsn = config["database_url"].as_str().unwrap();
        let url = reqwest::Url::parse(dsn).unwrap();
        assert_eq!(url.host_str(), Some("127.0.0.1"));
        assert_eq!(url.port(), Some(55485));
        assert_eq!(url.username(), "cbgpu_stage_mail");
        let pool = reviewed_pool(dsn)
            .await
            .expect("Narrow stage role must pass actual authority review");
        let lease = Uuid::new_v4();
        let job: Option<Value> = sqlx::query_scalar("select cbgpu_schools.claim_mail($1)")
            .bind(lease)
            .fetch_one(&pool)
            .await
            .unwrap();
        let job = job.expect("Synthetic feature request pending in isolated clone");
        use axum::{Json, Router, http::HeaderMap, routing::post};
        use std::sync::{Arc, Mutex};
        let keys = Arc::new(Mutex::new(Vec::<String>::new()));
        let seen = keys.clone();
        let app = Router::new().route(
            "/emails",
            post(move |h: HeaderMap, Json(v): Json<Value>| {
                let seen = seen.clone();
                async move {
                    assert_eq!(v["to"], json!(["info@cbgpu.com"]));
                    assert_eq!(v["from"], "CBGPU <info@cbgpu.com>");
                    seen.lock()
                        .unwrap()
                        .push(h["idempotency-key"].to_str().unwrap().to_owned());
                    Json(json!({"id":"synthetic-stage-accepted-not-delivered"}))
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap();
        for _ in 0..2 {
            assert_eq!(
                deliver(
                    &client,
                    &job,
                    "CBGPU <info@cbgpu.com>",
                    "synthetic-not-provider-key",
                    &format!("http://{addr}/emails")
                )
                .await,
                (true, "MAIL_PROVIDER_ACCEPTED")
            );
        }
        let saved = keys.lock().unwrap().clone();
        assert_eq!(saved.len(), 2);
        assert_eq!(saved[0], saved[1]);
        let id = Uuid::parse_str(job["id"].as_str().unwrap()).unwrap();
        let ack: bool = sqlx::query_scalar(
            "select cbgpu_schools.ack_mail($1,$2,true,'MAIL_PROVIDER_ACCEPTED')",
        )
        .bind(id)
        .bind(lease)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(ack);
        let next: Option<Value> = sqlx::query_scalar("select cbgpu_schools.claim_mail($1)")
            .bind(Uuid::new_v4())
            .fetch_one(&pool)
            .await
            .unwrap();
        assert!(next.is_none());
        task.abort();
    }
}
