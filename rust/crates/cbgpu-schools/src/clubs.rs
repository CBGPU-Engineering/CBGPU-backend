//! Only school-bound paths and bounded read pagination; SQL owns every permission/window.
use axum::http::Method;
use serde_json::{Value, json};
pub fn route(
    path: &[&str],
    method: &Method,
    query: &str,
) -> Result<Option<(&'static str, Value)>, ()> {
    let ["v1", "schools", school, "clubs", tail @ ..] = path else {
        return Ok(None);
    };
    let (action, mut value) = match (tail, method) {
        ([], &Method::GET) => ("clubs_list", json!({"school_id":school})),
        ([], &Method::POST) => ("clubs_create", json!({"school_id":school})),
        (["campaigns"], &Method::POST) => ("clubs_campaign_create", json!({"school_id":school})),
        (["campaigns", campaign], &Method::PATCH) => (
            "clubs_campaign_update",
            json!({"school_id":school,"campaign_id":campaign}),
        ),
        ([club], &Method::PATCH) => ("clubs_update", json!({"school_id":school,"club_id":club})),
        ([club, "ready"], &Method::POST) => {
            ("clubs_ready", json!({"school_id":school,"club_id":club}))
        }
        ([club, "withdraw"], &Method::POST) => {
            ("clubs_withdraw", json!({"school_id":school,"club_id":club}))
        }
        ([club, "applications"], &Method::GET) => (
            "clubs_applications",
            json!({"school_id":school,"club_id":club}),
        ),
        ([club, "applications"], &Method::POST) => {
            ("clubs_apply", json!({"school_id":school,"club_id":club}))
        }
        ([club, "applications", application], &Method::PATCH) => (
            "clubs_application_review",
            json!({"school_id":school,"club_id":club,"application_id":application}),
        ),
        ([club, "applications", application, "withdraw"], &Method::POST) => (
            "clubs_application_withdraw",
            json!({"school_id":school,"club_id":club,"application_id":application}),
        ),
        _ => return Ok(None),
    };
    if !query.is_empty() {
        if *method != Method::GET {
            return Err(());
        }
        let mut seen = std::collections::BTreeSet::new();
        for (key, v) in url::form_urlencoded::parse(query.as_bytes()) {
            if !seen.insert(key.to_string()) {
                return Err(());
            }
            match key.as_ref() {
                "limit" if v.parse::<u16>().is_ok_and(|v| (1..=100).contains(&v)) => {}
                "offset" if v.parse::<u32>().is_ok_and(|v| v <= 100000) => {}
                "campaign_id" if action == "clubs_list" && uuid::Uuid::parse_str(&v).is_ok() => {}
                _ => return Err(()),
            }
            value[key.as_ref()] = json!(v);
        }
    }
    Ok(Some((action, value)))
}
