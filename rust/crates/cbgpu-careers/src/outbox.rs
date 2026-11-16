//! Resend retries reuse the original idempotency key and Python-compatible body.
use crate::App;
use serde_json::Value;
use sqlx::Row;
use std::{path::Path, time::Duration};

fn python_string(value: &str) -> String {
    let encoded = serde_json::to_string(value).expect("string JSON");
    let mut result = String::new();
    for character in encoded.chars() {
        if character.is_ascii() {
            result.push(character);
        } else {
            let mut utf16 = [0; 2];
            for word in character.encode_utf16(&mut utf16) {
                result.push_str(&format!("\\u{word:04x}"));
            }
        }
    }
    result
}
pub fn mail_body(role: &str, reference: &str) -> String {
    let subject = format!("[CBGPU 지원 접수] {role} · {reference}");
    let html = format!(
        "<div style='font-family:Arial,sans-serif;max-width:560px;margin:auto;padding:32px'><img src='https://careers.cbgpu.com/logo.png' width='40' alt='CBGPU'><h1>새로운 동료의 지원서가 도착했어요.</h1><p>{role}</p><p>접수 번호: <b>{reference}</b></p><p>개인정보 보호를 위해 지원 내용은 메일에 포함하지 않았어요. 개발자 터미널에서 careers 관리 명령으로 확인해주세요.</p><a href='https://developers.cbgpu.com' style='background:#3182f6;color:white;padding:14px 20px;display:inline-block;border-radius:10px;text-decoration:none'>개발자 페이지 열기</a></div>"
    );
    format!(
        "{{\"from\": {}, \"to\": [\"info@cbgpu.com\"], \"subject\": {}, \"html\": {}}}",
        python_string("CBGPU Careers <info@cbgpu.com>"),
        python_string(&subject),
        python_string(&html)
    )
}
pub async fn load_key(path: &Path) -> Result<String, &'static str> {
    let contents = tokio::fs::read_to_string(path)
        .await
        .map_err(|_| "Cannot read careers mail configuration")?;
    let key = contents
        .lines()
        .filter(|line| !line.starts_with('#'))
        .filter_map(|line| line.split_once('='))
        .find(|(name, _)| *name == "SMTP_PASS")
        .map(|(_, value)| value.trim_matches(['\'', '"']).to_owned())
        .filter(|key| key.starts_with("re_"))
        .ok_or("Careers requires a Resend sending key")?;
    Ok(key)
}
pub async fn process(app: &App, key: &str, client: &reqwest::Client) -> Result<(), &'static str> {
    for _ in 0..10 {
        let mut tx = app
            .pool
            .begin()
            .await
            .map_err(|_| "Outbox database unavailable")?;
        let locked: bool = sqlx::query_scalar("SELECT pg_try_advisory_xact_lock(1128419925,3530)")
            .fetch_one(&mut *tx)
            .await
            .map_err(|_| "Outbox claim failed")?;
        if !locked {
            return Ok(());
        }
        let now = chrono::Utc::now().timestamp();
        sqlx::query("DELETE FROM careers.applications WHERE created<$1")
            .bind(now - 90 * 86400)
            .execute(&mut *tx)
            .await
            .map_err(|_| "Outbox retention failed")?;
        sqlx::query("DELETE FROM careers.requests WHERE created<$1")
            .bind(now - 86400)
            .execute(&mut *tx)
            .await
            .map_err(|_| "Outbox retention failed")?;
        let row=sqlx::query("SELECT id,role FROM careers.applications WHERE notified=0 ORDER BY created LIMIT 1 FOR UPDATE").fetch_optional(&mut *tx).await.map_err(|_|"Outbox read failed")?;
        let Some(row) = row else {
            tx.commit().await.map_err(|_| "Outbox commit failed")?;
            break;
        };
        let reference: String = row.try_get("id").map_err(|_| "Invalid outbox row")?;
        let role_id: String = row.try_get("role").map_err(|_| "Invalid outbox row")?;
        let role = app
            .roles
            .as_array()
            .into_iter()
            .flatten()
            .find(|role| role["id"] == role_id)
            .and_then(|role| role["name"].as_str())
            .unwrap_or(if role_id == "mobile" {
                "앱 개발 (이전 모집)"
            } else {
                "이전 모집 분야"
            });
        let mut response = client
            .post("https://api.resend.com/emails")
            .bearer_auth(key)
            .header("Content-Type", "application/json")
            .header("User-Agent", "CBGPU-Careers/1.0")
            .header("Idempotency-Key", format!("careers-{reference}"))
            .body(mail_body(role, &reference))
            .send()
            .await
            .map_err(|_| "Recruitment notification pending")?;
        if !response.status().is_success() {
            return Err("Recruitment notification pending");
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| "Recruitment acknowledgement unavailable")?
        {
            if bytes.len() + chunk.len() > 65536 {
                return Err("Recruitment acknowledgement too large");
            }
            bytes.extend_from_slice(&chunk);
        }
        let accepted: Value =
            serde_json::from_slice(&bytes).map_err(|_| "Recruitment acknowledgement invalid")?;
        if !accepted["id"].as_str().is_some_and(|id| !id.is_empty()) {
            return Err("Recruitment acknowledgement missing");
        }
        sqlx::query("UPDATE careers.applications SET notified=1 WHERE id=$1")
            .bind(reference)
            .execute(&mut *tx)
            .await
            .map_err(|_| "Outbox acknowledgement persistence failed")?;
        tx.commit().await.map_err(|_| "Outbox commit failed")?;
    }
    Ok(())
}
pub fn client() -> Result<reqwest::Client, &'static str> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| "Mail client initialization failed")
}
