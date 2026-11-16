use crate::{Result, Store, fail, safe_relative};
use serde_json::{Value, json};
pub const TEXT_MODELS: [&str; 4] = ["cbgpu-oss", "cbgpu-chat", "cbgpu-coder", "cbgpu-pro"];
fn chars(v: &str) -> usize {
    v.chars().count()
}
pub fn validate_chat(data: &Value) -> Result<Value> {
    let messages = data["messages"]
        .as_array()
        .ok_or_else(|| fail("Use text chat messages", 400))?;
    if !(1..=32).contains(&messages.len()) {
        return Err(fail("Use text chat messages", 400));
    }
    let mut length = 0;
    let mut cleaned = vec![];
    for m in messages {
        let role = m["role"].as_str().unwrap_or("");
        if !["system", "user", "assistant", "tool"].contains(&role)
            || !(m["content"].is_string()
                || role == "assistant" && m["content"].is_null() && m["tool_calls"].is_array())
        {
            return Err(fail("Use text chat messages", 400));
        }
        if role == "tool" && !m["tool_call_id"].is_string() {
            return Err(fail("Tool results require tool_call_id", 400));
        }
        let calls = serde_json::to_string(m.get("tool_calls").unwrap_or(&json!([])))?;
        if m.get("tool_calls")
            .is_some_and(|v| !v.is_null() && v != &json!([]))
            && (!m["tool_calls"].is_array() || calls.len() > 24000)
        {
            return Err(fail("Invalid tool history", 400));
        }
        length += chars(m["content"].as_str().unwrap_or("")) + calls.len();
        let mut n = json!({});
        for k in ["role", "content", "tool_calls", "tool_call_id"] {
            if let Some(v) = m.get(k) {
                n[k] = v.clone();
            }
        }
        cleaned.push(n);
    }
    if length > 48000 {
        return Err(fail("Text is too long", 413));
    }
    let model = match data.get("model") {
        None => "cbgpu-chat",
        Some(v) => v.as_str().ok_or_else(|| fail("Unknown chat model", 400))?,
    };
    if !TEXT_MODELS.contains(&model) {
        return Err(fail("Unknown chat model", 400));
    }
    let limit = data.get("max_tokens").and_then(Value::as_i64).unwrap_or(
        if data.get("max_tokens").is_none() {
            512
        } else {
            0
        },
    );
    if limit < 1 || limit > if model == "cbgpu-oss" { 4096 } else { 2048 } {
        return Err(fail("max_tokens exceeds the selected model limit", 400));
    }
    let effort = match data.get("effort") {
        None => "medium",
        Some(v) => v
            .as_str()
            .ok_or_else(|| fail("Use low, medium or high effort", 400))?,
    };
    if !["low", "medium", "high"].contains(&effort) {
        return Err(fail("Use low, medium or high effort", 400));
    }
    let tools = data.get("tools").cloned().unwrap_or(json!([]));
    validate_tools(&tools)?;
    let choice = data.get("tool_choice").cloned().unwrap_or(json!("auto"));
    let named = choice["type"] == "function"
        && choice["function"]["name"].as_str().is_some_and(|name| {
            tools
                .as_array()
                .is_some_and(|ts| ts.iter().any(|t| t["function"]["name"] == name))
        });
    if ![json!("auto"), json!("none"), json!("required")].contains(&choice) && !named {
        return Err(fail("Unsupported tool_choice", 400));
    }
    Ok(
        json!({"messages":cleaned,"max_tokens":limit,"model":model,"effort":effort,"tools":tools,"tool_choice":choice}),
    )
}
pub fn validate_tools(tools: &Value) -> Result<()> {
    let ts = tools
        .as_array()
        .ok_or_else(|| fail("Tools must be a list", 400))?;
    if ts.len() > 20 || tools.to_string().len() > 24000 {
        return Err(fail("Too many tools", 400));
    }
    let mut names = std::collections::HashSet::new();
    for t in ts {
        let f = &t["function"];
        let name = f["name"].as_str().unwrap_or("");
        if t["type"] != "function"
            || name.is_empty()
            || name.len() > 64
            || !name
                .as_bytes()
                .first()
                .is_some_and(|b| b.is_ascii_alphabetic() || *b == b'_')
            || !name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
            || !names.insert(name)
        {
            return Err(fail("Invalid tool function", 400));
        }
        if f.get("description")
            .is_some_and(|v| !v.is_string() || v.as_str().is_some_and(|s| chars(s) > 2000))
        {
            return Err(fail("Tool description is too long", 400));
        }
        if let Some(p) = f.get("parameters")
            && (!p.is_object()
                || p.get("type").is_some_and(|v| v != "object")
                || p.to_string().len() > 16000)
        {
            return Err(fail("Invalid tool parameters", 400));
        }
    }
    Ok(())
}
impl Store {
    pub async fn validate(&self, owner: &str, kind: &str, data: Value) -> Result<Value> {
        if !data.is_object() {
            return Err(fail("Unsupported task", 400));
        }
        match kind {
            "chat" => validate_chat(&data),
            "embed" => {
                let input = if let Some(s) = data["input"].as_str() {
                    vec![json!(s)]
                } else {
                    data["input"].as_array().cloned().ok_or_else(|| {
                        fail("Provide 1–64 texts of at most 8,000 characters", 400)
                    })?
                };
                if input.is_empty()
                    || input.len() > 64
                    || input
                        .iter()
                        .any(|v| v.as_str().is_none_or(|s| chars(s) > 8000))
                {
                    return Err(fail("Provide 1–64 texts of at most 8,000 characters", 400));
                }
                Ok(json!({"input":input}))
            }
            "search" => {
                let q = data["query"]
                    .as_str()
                    .ok_or_else(|| fail("Invalid search input", 400))?;
                let cs = data["candidates"]
                    .as_array()
                    .ok_or_else(|| fail("Invalid search input", 400))?;
                if q.trim().is_empty() || chars(q) > 1000 || cs.len() > 500 {
                    return Err(fail("Invalid search input", 400));
                }
                let mut candidates = vec![];
                let mut corpus = vec![];
                for c in cs {
                    let id = c["id"]
                        .as_str()
                        .ok_or_else(|| fail("Invalid search candidates", 400))?;
                    let text = c["text"]
                        .as_str()
                        .ok_or_else(|| fail("Invalid search candidates", 400))?;
                    if chars(id) > 150 || chars(text) > 8000 {
                        return Err(fail("Invalid search candidates", 400));
                    }
                    let mut clean = json!({"id":id,"text":text});
                    for (k, n) in [("label", 200), ("href", 500), ("snippet", 350)] {
                        clean[k] = json!(
                            c[k].as_str()
                                .unwrap_or("")
                                .chars()
                                .take(n)
                                .collect::<String>()
                        );
                    }
                    if id.starts_with("material:") || id.starts_with("paper:") {
                        corpus.push(id);
                    }
                    candidates.push(clean);
                }
                Ok(json!({"query":q,"candidates":candidates,"corpus_ids":corpus}))
            }
            "transcribe" | "document" => {
                let upload = self
                    .get_upload(owner, data["upload_id"].as_str().unwrap_or(""))
                    .await?;
                let language = data.get("language").cloned().unwrap_or(Value::Null);
                if ![
                    Value::Null,
                    json!("ko"),
                    json!("en"),
                    json!("ja"),
                    json!("zh"),
                ]
                .contains(&language)
                {
                    return Err(fail("Unsupported language", 400));
                }
                Ok(json!({"upload_id":upload["id"],"filename":upload["name"],"language":language}))
            }
            "python" => {
                let code = match data.get("code") {
                    None => "",
                    Some(v) => v
                        .as_str()
                        .ok_or_else(|| fail("Python source is too long", 400))?,
                };
                if chars(code) > 100000 {
                    return Err(fail("Python source is too long", 400));
                }
                let entry = data["entrypoint"].as_str().unwrap_or("main.py");
                safe_relative(entry)?;
                if !entry.ends_with(".py") {
                    return Err(fail("Entrypoint must be a Python file", 400));
                }
                let upload = data["upload_id"].as_str();
                if let Some(upload) = upload {
                    self.get_upload(owner, upload).await?;
                }
                if code.trim().is_empty() && upload.is_none() {
                    return Err(fail("Upload a ZIP or enter Python code", 400));
                }
                let minutes = data
                    .get("minutes")
                    .and_then(Value::as_i64)
                    .unwrap_or(if data.get("minutes").is_none() { 30 } else { 0 });
                if !(1..=120).contains(&minutes) {
                    return Err(fail("Duration must be 1–120 minutes", 400));
                }
                Ok(json!({"code":code,"upload_id":upload,"entrypoint":entry,"minutes":minutes}))
            }
            _ => Err(fail("Unsupported task", 400)),
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn validates_tools_and_model_limits() {
        let base = json!({"messages":[{"role":"user","content":"안녕"}],"model":"cbgpu-oss","max_tokens":4096});
        assert!(validate_chat(&base).is_ok());
        let mut bad = base;
        bad["model"] = json!("cbgpu-chat");
        assert!(validate_chat(&bad).is_err());
        assert!(
            validate_tools(&json!([{"type":"function","function":{"name":"../shell"}}])).is_err()
        );
    }
    #[test]
    fn tool_result_requires_call_id() {
        assert!(validate_chat(&json!({"messages":[{"role":"tool","content":"done"}]})).is_err());
    }
}
