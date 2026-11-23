use crate::{ContestBackend, RegistrationInput, Result, fail};
use serde_json::{Value, json};
const FIELDS: &str = "id,host_id,title,description,created_at,ends_at,is_open,fields,event_start_at,event_end_at,location_type,location_name,address,visibility,capacity,waitlist_enabled,approval_required,registration_open,registration_questions,registration_closes_at,lifecycle_status,cancellation_message,registration_opens_at,starts_at,participation_mode,team_min_size,team_max_size,allow_individual_entries,banner_path,school_id,access_scope,allowed_school_ids,participant_policy";
pub struct Backend {
    pub origin: String,
    pub anon_key: String,
    pub client: reqwest::Client,
}
impl Backend {
    fn request(&self, method: reqwest::Method, path: &str, token: &str) -> reqwest::RequestBuilder {
        self.client
            .request(method, format!("{}/rest/v1/{path}", self.origin))
            .header("apikey", &self.anon_key)
            .bearer_auth(token)
    }
    async fn value(request: reqwest::RequestBuilder) -> Result<Value> {
        let mut response = request
            .send()
            .await
            .map_err(|_| fail(503, "backend_unavailable", "Contest database unavailable"))?;
        let status = response.status();
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| fail(503, "backend_unavailable", "Contest response unavailable"))?
        {
            if bytes.len() + chunk.len() > 8 * 1024 * 1024 {
                return Err(fail(
                    503,
                    "backend_unavailable",
                    "Contest response exceeds the safe limit",
                ));
            }
            bytes.extend_from_slice(&chunk)
        }
        let value: Value = serde_json::from_slice(&bytes)
            .map_err(|_| fail(503, "backend_unavailable", "Invalid contest response"))?;
        if !status.is_success() {
            return Err(database_error(status.as_u16(), &value));
        }
        Ok(value)
    }
    fn own_row(value: Value, id: uuid::Uuid, subject: uuid::Uuid) -> Result<Option<Value>> {
        let rows = value
            .as_array()
            .ok_or_else(|| fail(503, "backend_unavailable", "Invalid registration response"))?;
        if rows.len() > 1 {
            return Err(fail(
                503,
                "backend_unavailable",
                "Ambiguous own registration",
            ));
        }
        match rows.first() {
            None => Ok(None),
            Some(row)
                if row["contest_id"]
                    .as_str()
                    .and_then(|s| uuid::Uuid::parse_str(s).ok())
                    == Some(id)
                    && row["user_id"]
                        .as_str()
                        .and_then(|s| uuid::Uuid::parse_str(s).ok())
                        == Some(subject) =>
            {
                Ok(Some(row.clone()))
            }
            _ => Err(fail(
                503,
                "backend_unavailable",
                "Registration ownership mismatch",
            )),
        }
    }
}
pub fn database_error(status: u16, value: &Value) -> crate::Error {
    let code = value["code"].as_str().unwrap_or("");
    let raw = value["message"].as_str().unwrap_or("");
    let message = raw.chars().take(500).collect::<String>();
    if status == 401 {
        return fail(
            401,
            "unauthorized",
            "CBGPU access JWT expired or was rejected",
        );
    }
    if status == 403 || code == "42501" {
        return fail(403, "permission_denied", "Contest permission denied");
    }
    if code == "23505" || raw.contains("이미 참가 신청") {
        return fail(
            409,
            "already_registered",
            if message.is_empty() {
                "A registration already exists"
            } else {
                &message
            },
        );
    }
    if raw.contains("NOT_STARTED") {
        return fail(
            409,
            "not_started",
            "Contest registration has not opened yet",
        );
    }
    if raw.contains("CLOSED") || raw.contains("마감") {
        return fail(409, "registration_closed", "Contest registration is closed");
    }
    if raw == "EVENT_FULL" {
        return fail(
            409,
            "capacity_reached",
            "Contest is full and its waitlist is disabled",
        );
    }
    if ["P0001", "23514", "22023"].contains(&code) {
        return fail(
            400,
            "registration_rejected",
            if message.is_empty() {
                "Contest rejected this registration"
            } else {
                &message
            },
        );
    }
    fail(
        503,
        "backend_unavailable",
        "Contest database could not complete the request",
    )
}
#[async_trait::async_trait]
impl ContestBackend for Backend {
    async fn list(&self, token: &str, limit: u16, offset: u32) -> Result<Vec<Value>> {
        let value = Self::value(
            self.request(reqwest::Method::GET, "contests", token)
                .query(&[
                    ("select", FIELDS),
                    ("visibility", "eq.public"),
                    ("lifecycle_status", "eq.published"),
                    ("order", "created_at.desc"),
                    ("limit", &limit.to_string()),
                    ("offset", &offset.to_string()),
                ]),
        )
        .await?;
        serde_json::from_value(value)
            .map_err(|_| fail(503, "backend_unavailable", "Invalid contest list"))
    }
    async fn detail(&self, token: &str, id: uuid::Uuid) -> Result<Value> {
        let value = Self::value(
            self.request(reqwest::Method::GET, "contests", token)
                .query(&[
                    ("select", FIELDS),
                    ("id", &format!("eq.{id}")),
                    ("limit", "1"),
                ]),
        )
        .await?;
        value
            .as_array()
            .and_then(|v| v.first())
            .filter(|v| v["id"].as_str().and_then(|s| uuid::Uuid::parse_str(s).ok()) == Some(id))
            .cloned()
            .ok_or_else(|| fail(404, "not_found", "Contest not found"))
    }
    async fn register(
        &self,
        token: &str,
        id: uuid::Uuid,
        input: &RegistrationInput,
    ) -> Result<Value> {
        let value=Self::value(self.request(reqwest::Method::POST,"rpc/register_contest_entry",token).json(&json!({"p_contest_id":id,"p_attendee_name":input.attendee_name,"p_answers":input.answers,"p_kind":input.kind,"p_team_name":input.team_name,"p_join_code":input.join_code}))).await?;
        value
            .as_array()
            .filter(|v| v.len() == 1)
            .and_then(|v| v.first())
            .filter(|v| {
                v["contest_id"]
                    .as_str()
                    .and_then(|s| uuid::Uuid::parse_str(s).ok())
                    == Some(id)
            })
            .cloned()
            .ok_or_else(|| fail(503, "backend_unavailable", "Invalid registration result"))
    }
    async fn own(&self, token: &str, id: uuid::Uuid, subject: uuid::Uuid) -> Result<Option<Value>> {
        let value = Self::value(
            self.request(reqwest::Method::GET, "event_registrations", token)
                .query(&[
                    ("select", "*"),
                    ("contest_id", &format!("eq.{id}")),
                    ("user_id", &format!("eq.{subject}")),
                    ("limit", "1"),
                ]),
        )
        .await?;
        Self::own_row(value, id, subject)
    }
    async fn cancel(
        &self,
        token: &str,
        id: uuid::Uuid,
        subject: uuid::Uuid,
    ) -> Result<Option<Value>> {
        if let Some(current) = self.own(token, id, subject).await? {
            if current["status"] == "cancelled" {
                return Ok(Some(current));
            }
        } else {
            return Ok(None);
        }
        let value = Self::value(
            self.request(reqwest::Method::PATCH, "event_registrations", token)
                .query(&[
                    ("contest_id", format!("eq.{id}")),
                    ("user_id", format!("eq.{subject}")),
                ])
                .header("prefer", "return=representation")
                .json(&json!({"status":"cancelled"})),
        )
        .await?;
        Self::own_row(value, id, subject)
    }
}
