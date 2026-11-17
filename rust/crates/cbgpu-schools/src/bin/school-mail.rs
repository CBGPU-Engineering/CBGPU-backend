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
    let url = format!("https://cbgpu.com/schools/{id}/admin");
    let html = format!(
        r#"<!doctype html><html lang="ko"><meta charset="utf-8"><body style="margin:0;background:#f4f6f8;font-family:Arial,sans-serif;color:#191f28"><table role="presentation" style="width:100%;padding:28px"><tr><td><div style="max-width:600px;margin:auto;background:white;padding:32px;border-radius:16px"><a href="https://cbgpu.com"><img src="https://cbgpu.com/brand/v1/symbol_black.png" alt="CBGPU" width="40" height="40"></a><h1 style="font-size:24px">학교 전용 기능 요청</h1><p>{}</p><h2 style="font-size:18px">{}</h2><p style="line-height:1.8">{}</p><p style="font-size:12px;color:#667085">요청 번호: {}</p><a href="{}" style="display:inline-block;background:#17634d;color:white;text-decoration:none;border-radius:8px;padding:14px 20px">학교 관리 화면 열기</a><p style="font-size:13px;color:#667085">이 메일은 요청 접수 알림입니다. 기능 승인 또는 개발 완료를 의미하지 않습니다.</p><p style="font-size:12px">CBGPU · info@cbgpu.com</p></div></td></tr></table></body></html>"#,
        escape(school),
        escape(title),
        escape(description).replace('\n', "<br>"),
        escape(request),
        escape(&url)
    );
    Ok(
        json!({"from":from,"to":["info@cbgpu.com"],"subject":subject,"html":html,"text":format!("학교 전용 기능 요청\n학교: {school}\n제목: {title}\n\n{description}\n\n요청 번호: {request}\n{url}\n요청 접수는 승인이나 구현 완료를 의미하지 않습니다.")}),
    )
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::var("CBGPU_SCHOOL_MAIL_ENABLED").as_deref() != Ok("1") {
        return Err("School mail worker is disabled".into());
    }
    let api_key = std::env::var("RESEND_API_KEY")?;
    let from = std::env::var("CBGPU_EMAIL_FROM").unwrap_or("CBGPU <info@cbgpu.com>".into());
    if api_key.trim().is_empty() || from.contains(['\r', '\n']) {
        return Err("Invalid private mail configuration".into());
    }
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .acquire_timeout(std::time::Duration::from_secs(10))
        .connect(&std::env::var("CBGPU_SCHOOL_MAIL_DATABASE_URL")?)
        .await?;
    let safe:bool=sqlx::query_scalar("SELECT NOT r.rolsuper AND NOT r.rolbypassrls AND pg_has_role(current_user,'cbgpu_school_mail_worker','member') AND NOT has_table_privilege(current_user,'auth.users','SELECT,INSERT,UPDATE,DELETE') AND NOT has_table_privilege(current_user,'cbgpu_schools.mail_outbox','SELECT,INSERT,UPDATE,DELETE') AND NOT has_table_privilege(current_user,'cbgpu_schools.memberships','SELECT,INSERT,UPDATE,DELETE') AND has_function_privilege(current_user,'cbgpu_schools.claim_mail(uuid)','EXECUTE') AND has_function_privilege(current_user,'cbgpu_schools.ack_mail(uuid,uuid,boolean,text)','EXECUTE') FROM pg_roles r WHERE r.rolname=current_user").fetch_one(&pool).await?;
    if !safe {
        return Err("Mail runtime must use reviewed EXECUTE-only credentials".into());
    }
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(20))
        .build()?;
    let once = std::env::args().any(|a| a == "--once");
    loop {
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
            let (mut success, mut code) = (false, "MAIL_PROVIDER_UNAVAILABLE");
            match payload(&job, &from) {
                Ok(body) => {
                    if let Ok(response) = client
                        .post("https://api.resend.com/emails")
                        .bearer_auth(&api_key)
                        .header("Idempotency-Key", format!("cbgpu-school-mail-{id}"))
                        .json(&body)
                        .send()
                        .await
                    {
                        let status = response.status();
                        code = if status.as_u16() == 429 {
                            "MAIL_PROVIDER_RATE_LIMIT"
                        } else if status.is_client_error() {
                            "MAIL_PROVIDER_REJECTED"
                        } else {
                            "MAIL_PROVIDER_UNAVAILABLE"
                        };
                        if status.is_success()
                            && let Ok(body) = response.json::<Value>().await
                        {
                            success = body["id"].as_str().is_some_and(|s| !s.is_empty());
                            if success {
                                code = "MAIL_PROVIDER_ACCEPTED"
                            }
                        }
                    }
                }
                Err(error) => code = error,
            }
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
        tokio::select! {_=tokio::signal::ctrl_c()=>break,_=tokio::time::sleep(std::time::Duration::from_secs(5))=>{}}
    }
    Ok(())
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
        json!({"recipient":"info@cbgpu.com","template":"school-feature-request","school_name":"학교 <script>","school_id":"school-example","title":"요청\r\nInjected: header","description":"<img src=x onerror=alert(1)>\n둘째 줄","request_id":"218bf41f-c40c-4497-a170-52c642880d8b"})
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
}
