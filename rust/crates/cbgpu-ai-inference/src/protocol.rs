use crate::native::{Model, Result};
use serde_json::{Value, json};
use std::collections::BTreeMap;
fn content(message: &Value) -> String {
    message["content"].as_str().unwrap_or("").into()
}
pub fn prepare(messages: &[Value], tools: &[Value], choice: &Value) -> (Vec<Value>, bool) {
    let mut output = vec![];
    for message in messages {
        let mut role = message["role"].as_str().unwrap_or("user");
        let mut value = content(message);
        if role == "tool" {
            role = "user";
            value = format!(
                "Tool result for {}:\n{}",
                message["tool_call_id"].as_str().unwrap_or(""),
                value
            );
        }
        if message["tool_calls"].is_array() {
            value.push_str(&format!("\nRequested tools: {}", message["tool_calls"]));
        }
        output.push(json!({"role":role,"content":value}));
    }
    if tools.is_empty() || choice == "none" {
        return (output, false);
    }
    let required = choice == "required" || choice.is_object();
    let mut instruction = format!(
        "You are a coding agent with these available function tools. Never claim a tool ran before its result is provided. Return EXACTLY one JSON object. To request a tool: {{\"type\":\"tool\",\"name\":\"FUNCTION_NAME\",\"arguments\":{{...}}}}. To answer without a tool: {{\"type\":\"final\",\"content\":\"your answer\"}}. {}Tools:\n{}",
        if required {
            "You MUST request one tool this turn. "
        } else {
            ""
        },
        json!(tools)
    );
    if choice.is_object() {
        instruction.push_str(&format!(
            "\nUse only function: {}",
            choice["function"]["name"].as_str().unwrap_or("")
        ));
    }
    for message in messages {
        if let Some(root) = content(message).lines().find_map(|line| {
            line.split_once("CBGPU_PROJECT_ROOT: ")
                .map(|(_, path)| path.to_owned())
        }) {
            instruction = format!(
                "Actual project root: {root}. Use this real path, never invent /path/to paths.\n{instruction}"
            );
            break;
        }
    }
    output.insert(0, json!({"role":"system","content":instruction}));
    (output, true)
}
pub fn decode(raw: &str, tools: &[Value], choice: &Value) -> Result<(Value, &'static str)> {
    let raw: Value = serde_json::from_str(raw)
        .map_err(|_| "Model did not return the requested tool/answer format")?;
    match raw["type"].as_str() {
        Some("tool") => {
            let name = raw["name"]
                .as_str()
                .ok_or("Model returned an invalid tool proposal")?;
            if choice == "none"
                || !tools.iter().any(|tool| tool["function"]["name"] == name)
                || !raw["arguments"].is_object()
                || choice.is_object() && choice["function"]["name"] != name
            {
                return Err("Model returned an invalid tool proposal".into());
            }
            Ok((
                json!({"role":"assistant","content":Value::Null,"tool_calls":[{"id":format!("call_{}",&uuid::Uuid::new_v4().simple().to_string()[..24]),"type":"function","function":{"name":name,"arguments":raw["arguments"].to_string()}}]}),
                "tool_calls",
            ))
        }
        Some("final")
            if choice != "required" && !choice.is_object() && raw["content"].is_string() =>
        {
            Ok((json!({"role":"assistant","content":raw["content"]}), "stop"))
        }
        _ => Err("Model did not return the requested tool/answer format".into()),
    }
}
pub fn chatml(
    model: &Model,
    messages: &[Value],
    max_tokens: usize,
    effort: &str,
    tool_mode: bool,
) -> Result<(String, bool)> {
    let review = effort == "high" && !tool_mode;
    let mut budget = 7000usize.saturating_sub(max_tokens + if review { 768 } else { 0 });
    let mut prepared = vec![];
    let mut truncated = false;
    let systems = messages
        .iter()
        .filter(|m| m["role"] == "system")
        .map(content)
        .collect::<Vec<_>>()
        .join("\n\n");
    if !systems.is_empty() {
        let mut tokens = model.tokenize(&systems, false)?;
        let cap = budget * 65 / 100;
        if tokens.len() > cap {
            tokens.truncate(cap);
            truncated = true;
        }
        budget = budget.saturating_sub(tokens.len());
        prepared.push(json!({"role":"system","content":model.text(&tokens)?}));
    }
    let mut recent = vec![];
    for message in messages.iter().rev().filter(|m| m["role"] != "system") {
        let mut tokens = model.tokenize(&content(message), false)?;
        if tokens.len() > budget {
            truncated = true;
            if !recent.is_empty() {
                break;
            }
            tokens.truncate(budget);
        }
        budget = budget.saturating_sub(tokens.len());
        recent.push(json!({"role":message["role"],"content":model.text(&tokens)?}));
        if budget == 0 {
            truncated = true;
            break;
        }
    }
    recent.reverse();
    prepared.extend(recent);
    if effort == "medium" {
        prepared.insert(0,json!({"role":"system","content":"답변의 정확성과 사용자의 제약을 확인하고 명확한 최종 답변을 제시하세요."}));
    }
    Ok((format_chatml(&prepared), truncated))
}
pub fn format_chatml(messages: &[Value]) -> String {
    let mut prompt = String::new();
    for m in messages {
        prompt.push_str(&format!(
            "<|im_start|>{}\n{}<|im_end|>\n",
            m["role"].as_str().unwrap_or("user"),
            content(m)
        ));
    }
    prompt.push_str("<|im_start|>assistant\n");
    prompt
}
fn schema_type(schema: &Value, depth: usize) -> String {
    if depth > 5 {
        return "unknown".into();
    }
    if let Some(values) = schema["enum"].as_array() {
        return values
            .iter()
            .take(30)
            .map(Value::to_string)
            .collect::<Vec<_>>()
            .join(" | ");
    }
    match schema["type"].as_str() {
        Some("integer" | "number") => "number".into(),
        Some(t @ ("string" | "boolean" | "null")) => t.into(),
        Some("array") => format!("Array<{}>", schema_type(&schema["items"], depth + 1)),
        Some("object") => {
            let required = schema["required"].as_array();
            format!(
                "{{{}}}",
                schema["properties"]
                    .as_object()
                    .map(|properties| properties
                        .iter()
                        .map(|(key, value)| format!(
                            "{}{}:{}",
                            json!(key),
                            if required.is_some_and(|r| r.contains(&json!(key))) {
                                ""
                            } else {
                                "?"
                            },
                            schema_type(value, depth + 1)
                        ))
                        .collect::<Vec<_>>()
                        .join(","))
                    .unwrap_or_default()
            )
        }
        _ => {
            if let Some(values) = schema["anyOf"]
                .as_array()
                .or_else(|| schema["oneOf"].as_array())
            {
                values
                    .iter()
                    .take(10)
                    .map(|v| schema_type(v, depth + 1))
                    .collect::<Vec<_>>()
                    .join(" | ")
            } else {
                "unknown".into()
            }
        }
    }
}
pub struct Harmony {
    pub prompt: String,
    pub aliases: BTreeMap<String, String>,
    pub truncated: bool,
}
pub fn harmony(
    model: &Model,
    messages: &[Value],
    tools: &[Value],
    choice: &Value,
    effort: &str,
    max_tokens: usize,
) -> Result<Harmony> {
    let enabled = !tools.is_empty() && choice != "none";
    let mut aliases = BTreeMap::new();
    for (i, tool) in tools.iter().enumerate() {
        let name = tool["function"]["name"].as_str().ok_or("Invalid tool")?;
        let alias = format!(
            "f{i}_{}",
            name.chars()
                .map(|c| if c.is_ascii_alphanumeric() || c == '_' {
                    c
                } else {
                    '_'
                })
                .collect::<String>()
        );
        aliases.insert(alias, name.to_owned());
    }
    let names: BTreeMap<_, _> = aliases
        .iter()
        .map(|(a, n)| (n.clone(), a.clone()))
        .collect();
    let mut system = format!("You are a helpful assistant.\nReasoning: {effort}\n");
    if enabled {
        system.push_str("Calls to these tools must go to the commentary channel: 'functions'.\n");
    }
    system.push_str(
        &messages
            .iter()
            .filter(|m| m["role"] == "system")
            .map(content)
            .collect::<Vec<_>>()
            .join("\n\n"),
    );
    if choice == "required" {
        system.push_str("\nYou must call one available function this turn.");
    }
    if choice.is_object() {
        let name = choice["function"]["name"]
            .as_str()
            .ok_or("Invalid required function")?;
        let alias = names.get(name).ok_or("Required function is unavailable")?;
        system.push_str(&format!("\nCall only functions.{alias} this turn."));
    }
    let mut declarations = "namespace functions {\n".to_owned();
    for tool in tools {
        let f = &tool["function"];
        let name = f["name"].as_str().ok_or("Invalid function")?;
        declarations.push_str(&format!(
            "// {}\ntype {} = (_: {}) => any;\n",
            f["description"]
                .as_str()
                .unwrap_or("")
                .replace('\n', " ")
                .chars()
                .take(200)
                .collect::<String>(),
            names.get(name).ok_or("Invalid function")?,
            schema_type(&f["parameters"], 0)
        ));
    }
    declarations.push_str("} // namespace functions");
    let mut budget = 8192usize.saturating_sub(max_tokens + 200);
    let declaration_tokens = if enabled {
        model.tokenize(&declarations, false)?
    } else {
        vec![]
    };
    if declaration_tokens.len() > budget * 55 / 100 {
        return Err("Too many tool definitions for this model context".into());
    }
    let mut system_tokens = model.tokenize(&system, false)?;
    let cap = system_tokens.len().min(
        (budget * 7 / 10)
            .saturating_sub(declaration_tokens.len())
            .max(100),
    );
    let mut truncated = system_tokens.len() > cap;
    system_tokens.truncate(cap);
    budget = budget.saturating_sub(system_tokens.len() + declaration_tokens.len());
    let mut prompt = format!(
        "<|start|>system<|message|>{}<|end|>",
        model.text(&system_tokens)?
    );
    if enabled {
        prompt.push_str(&format!(
            "<|start|>developer<|message|>{declarations}<|end|>"
        ));
    }
    let mut segments = vec![];
    let mut call_names = BTreeMap::new();
    for message in messages {
        let role = message["role"].as_str().unwrap_or("user");
        if role == "system" {
            continue;
        }
        if role == "assistant" && message["tool_calls"].is_array() {
            for call in message["tool_calls"].as_array().unwrap() {
                let alias = names
                    .get(call["function"]["name"].as_str().unwrap_or(""))
                    .map(String::as_str)
                    .unwrap_or("legacy_tool");
                call_names.insert(
                    call["id"].as_str().unwrap_or("").to_owned(),
                    alias.to_owned(),
                );
                segments.push((
                    format!("assistant to=functions.{alias}<|channel|>commentary<|constrain|>json"),
                    call["function"]["arguments"]
                        .as_str()
                        .unwrap_or("{}")
                        .to_owned(),
                    "<|call|>",
                ));
            }
        } else if role == "tool" {
            segments.push((
                format!(
                    "functions.{} to=assistant<|channel|>commentary",
                    call_names
                        .get(message["tool_call_id"].as_str().unwrap_or(""))
                        .map(String::as_str)
                        .unwrap_or("legacy_tool")
                ),
                content(message),
                "<|end|>",
            ));
        } else {
            segments.push((
                if role == "assistant" {
                    "assistant<|channel|>final".into()
                } else {
                    "user".into()
                },
                content(message),
                "<|end|>",
            ));
        }
    }
    let mut recent = vec![];
    for (header, text, end) in segments.into_iter().rev() {
        let mut tokens = model.tokenize(&text, false)?;
        if tokens.len() > budget {
            truncated = true;
            if !recent.is_empty() {
                break;
            }
            tokens.truncate(budget);
        }
        budget = budget.saturating_sub(tokens.len());
        recent.push(format!(
            "<|start|>{header}<|message|>{}{end}",
            model.text(&tokens)?
        ));
        if budget == 0 {
            truncated = true;
            break;
        }
    }
    recent.reverse();
    prompt.push_str(&recent.join(""));
    prompt.push_str("<|start|>assistant");
    Ok(Harmony {
        prompt,
        aliases,
        truncated,
    })
}
pub fn visible_harmony(raw: &str) -> Option<String> {
    let text = raw.split_once("<|channel|>final<|message|>")?.1;
    let end = ["<|return|>", "<|fim_suffix|>", "<|end|>", "<|start|>"]
        .iter()
        .filter_map(|tag| text.find(tag))
        .min()
        .unwrap_or(text.len());
    let visible = &text[..end];
    // A special token may span several model pieces. Keep its unfinished suffix
    // out of partial.json until it is known to be ordinary text.
    let held = [
        "<|return|>",
        "<|fim_suffix|>",
        "<|end|>",
        "<|start|>",
        "<|call|>",
    ]
    .iter()
    .flat_map(|tag| (1..tag.len()).map(move |n| &tag[..n]))
    .filter(|prefix| visible.ends_with(prefix))
    .map(str::len)
    .max()
    .unwrap_or(0);
    Some(visible[..visible.len() - held].trim_start().to_owned())
}
pub fn harmony_result(
    raw: &str,
    aliases: &BTreeMap<String, String>,
    choice: &Value,
) -> Result<(Value, &'static str)> {
    let current = raw.rsplit("<|start|>assistant").next().unwrap_or(raw);
    if let Some((header, body)) = current.split_once("<|message|>") {
        let re = regex::Regex::new(r"to=functions\.([A-Za-z0-9_]+)").expect("constant tool regex");
        if let Some(capture) = re.captures(header) {
            let name = aliases
                .get(&capture[1])
                .ok_or("Unknown GPT-OSS tool proposal")?;
            let end = ["<|call|>", "<|end|>", "<|return|>", "<|fim_suffix|>"]
                .iter()
                .filter_map(|tag| body.find(tag))
                .min()
                .unwrap_or(body.len());
            let arguments: Value = serde_json::from_str(body[..end].trim())
                .map_err(|_| "Tool arguments must be an object")?;
            if choice == "none"
                || !arguments.is_object()
                || choice.is_object() && choice["function"]["name"] != name.as_str()
            {
                return Err("Model selected an invalid required tool".into());
            }
            return Ok((
                json!({"role":"assistant","content":Value::Null,"tool_calls":[{"id":format!("call_{}",&uuid::Uuid::new_v4().simple().to_string()[..24]),"type":"function","function":{"name":name,"arguments":arguments.to_string()}}]}),
                "tool_calls",
            ));
        }
    }
    if choice == "required" || choice.is_object() {
        return Err("Model did not return the required function".into());
    }
    let value = visible_harmony(raw).ok_or(
        "추론 예산 안에 최종 답변을 완성하지 못했어요. 노력 정도를 낮추거나 질문을 나눠 주세요.",
    )?;
    Ok((json!({"role":"assistant","content":value.trim()}), "stop"))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tools_are_proposals_never_execution_and_required_selection_is_enforced() {
        let tools = vec![
            json!({"type":"function","function":{"name":"read_file","parameters":{"type":"object"}}}),
        ];
        let (m, finish) = decode(
            r#"{"type":"tool","name":"read_file","arguments":{"path":"/project/a"}}"#,
            &tools,
            &json!("required"),
        )
        .unwrap();
        assert_eq!(finish, "tool_calls");
        assert!(m["content"].is_null());
        assert!(
            decode(
                r#"{"type":"tool","name":"delete_everything","arguments":{}}"#,
                &tools,
                &json!("auto")
            )
            .is_err()
        );
        assert!(
            decode(
                r#"{"type":"final","content":"done"}"#,
                &tools,
                &json!("required")
            )
            .is_err()
        );
    }
    #[test]
    fn harmony_never_streams_analysis_or_unvalidated_calls() {
        assert!(visible_harmony("<|channel|>analysis<|message|>private reasoning").is_none());
        assert_eq!(
            visible_harmony(
                "analysis hidden<|channel|>final<|message|> Hello 한글<|return|>garbage"
            ),
            Some("Hello 한글".into())
        );
        let aliases = BTreeMap::from([("f0_read_file".into(), "read_file".into())]);
        let (m, f) = harmony_result(
            " to=functions.f0_read_file<|channel|>commentary<|message|>{\"path\":\"x\"}<|call|>",
            &aliases,
            &json!("required"),
        )
        .unwrap();
        assert_eq!(f, "tool_calls");
        assert_eq!(m["tool_calls"][0]["function"]["name"], "read_file");
        assert!(
            harmony_result(
                " to=functions.unknown<|message|>{}",
                &aliases,
                &json!("auto")
            )
            .is_err()
        );
    }
    #[test]
    fn tool_history_and_actual_project_root_survive_prompt_preparation() {
        let (m, enabled) = prepare(
            &[
                json!({"role":"system","content":"CBGPU_PROJECT_ROOT: /real/project"}),
                json!({"role":"tool","tool_call_id":"abc","content":"result"}),
            ],
            &[json!({"function":{"name":"read_file"}})],
            &json!("auto"),
        );
        assert!(enabled);
        assert!(m[0]["content"].as_str().unwrap().contains("/real/project"));
        assert_eq!(m[2]["role"], "user");
        assert!(m[2]["content"].as_str().unwrap().contains("abc"));
    }
    #[test]
    fn split_control_tokens_do_not_leak_into_stream_and_tools_none_rejects_calls() {
        let prefix = "<|channel|>final<|message|>한글";
        for token in ["<|return|>", "<|fim_suffix|>", "<|end|>", "<|start|>"] {
            for i in 1..token.len() {
                assert_eq!(
                    visible_harmony(&format!("{prefix}{}", &token[..i])),
                    Some("한글".into())
                );
            }
        }
        let tools = vec![json!({"function":{"name":"read_file"}})];
        assert!(
            decode(
                r#"{"type":"tool","name":"read_file","arguments":{}}"#,
                &tools,
                &json!("none")
            )
            .is_err()
        );
        assert!(
            harmony_result(
                " to=functions.f0<|message|>{}<|call|>",
                &BTreeMap::from([("f0".into(), "read_file".into())]),
                &json!("none")
            )
            .is_err()
        );
    }
}
