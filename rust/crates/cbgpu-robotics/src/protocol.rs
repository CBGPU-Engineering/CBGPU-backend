use crate::{Result, fail, require, valid_id};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
pub const VERSION: u8 = 1;
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Capability {
    pub name: String,
    pub description: String,
    pub parameters: Value,
    pub max_duration_s: f64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Step {
    pub capability: String,
    pub arguments: Value,
    pub duration_s: f64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Proposal {
    pub summary: String,
    pub needs_clarification: bool,
    pub question: String,
    pub steps: Vec<Step>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Mission {
    pub protocol_version: u8,
    pub id: String,
    pub robot_id: String,
    pub capabilities_digest: String,
    pub issued_at: f64,
    pub expires_at: f64,
    pub observed_at: f64,
    pub requires_local_approval: bool,
    pub plan: Proposal,
}
fn length(value: &str) -> usize {
    value.chars().count()
}
fn finite(value: &Value) -> Option<f64> {
    value.as_f64().filter(|v| v.is_finite())
}
fn uint(value: Option<&Value>) -> Option<u64> {
    value.and_then(Value::as_u64)
}
fn object(value: &Value) -> Result<&Map<String, Value>> {
    value
        .as_object()
        .ok_or_else(|| fail("Invalid parameter schema", 400))
}
/// Python's JSON float formatting is retained for the cross-language capabilities digest.
fn python_number(value: &serde_json::Number) -> String {
    if !value.is_f64() {
        return value.to_string();
    }
    let v = value.as_f64().unwrap_or(0.0);
    if v == 0.0 {
        return if v.is_sign_negative() { "-0.0" } else { "0.0" }.into();
    }
    let negative = v.is_sign_negative();
    let mut buffer = ryu::Buffer::new();
    let raw = buffer.format_finite(v.abs());
    let (mantissa, exponent) = raw
        .split_once('e')
        .map(|(m, e)| (m, e.parse::<i32>().unwrap_or(0)))
        .unwrap_or((raw, 0));
    let before = mantissa.find('.').unwrap_or(mantissa.len()) as i32;
    let mut digits = mantissa.replace('.', "");
    let leading = digits.bytes().take_while(|b| *b == b'0').count();
    digits.drain(..leading);
    let exponent = exponent + before - leading as i32 - 1;
    while digits.ends_with('0') && digits.len() > 1 {
        digits.pop();
    }
    let body = if !(-4..16).contains(&exponent) {
        let rest = &digits[1..];
        format!(
            "{}{}e{}{:02}",
            &digits[..1],
            if rest.is_empty() {
                String::new()
            } else {
                format!(".{rest}")
            },
            if exponent >= 0 { '+' } else { '-' },
            exponent.abs()
        )
    } else if exponent < 0 {
        format!("0.{}{}", "0".repeat((-exponent - 1) as usize), digits)
    } else {
        let split = (exponent + 1) as usize;
        if split >= digits.len() {
            format!("{}{}.0", digits, "0".repeat(split - digits.len()))
        } else {
            format!("{}.{}", &digits[..split], &digits[split..])
        }
    };
    if negative { format!("-{body}") } else { body }
}
fn json_string(value: &Value, compact: bool) -> String {
    match value {
        Value::Number(value) => python_number(value),
        Value::Array(values) => format!(
            "[{}]",
            values
                .iter()
                .map(|v| json_string(v, compact))
                .collect::<Vec<_>>()
                .join(if compact { "," } else { ", " })
        ),
        Value::Object(values) => format!(
            "{{{}}}",
            values
                .iter()
                .map(|(key, value)| format!(
                    "{}{}{}",
                    serde_json::to_string(key).expect("JSON key"),
                    if compact { ":" } else { ": " },
                    json_string(value, compact)
                ))
                .collect::<Vec<_>>()
                .join(if compact { "," } else { ", " })
        ),
        _ => serde_json::to_string(value).expect("finite JSON"),
    }
}
pub fn bounded(value: &Value, limit: usize, name: &str) -> Result<String> {
    let text = json_string(value, false);
    require(text.len() <= limit, &format!("{name} is too large"))?;
    Ok(text)
}
pub fn schema_check(schema: &Value, depth: usize) -> Result<()> {
    let s = object(schema)?;
    require(depth <= 4, "Invalid parameter schema")?;
    let kind = s
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| fail("Unsupported schema type", 400))?;
    let mut allowed = BTreeSet::from(["type", "description", "enum"]);
    match kind {
        "object" => {
            allowed.extend(["properties", "required", "additionalProperties"]);
            let properties = s
                .get("properties")
                .and_then(Value::as_object)
                .ok_or_else(|| {
                    fail(
                        "Objects need properties and additionalProperties: false",
                        400,
                    )
                })?;
            require(
                properties.len() <= 16
                    && s.get("additionalProperties") == Some(&Value::Bool(false)),
                "Objects need properties and additionalProperties: false",
            )?;
            let required = s
                .get("required")
                .cloned()
                .unwrap_or_else(|| Value::Array(vec![]));
            let required = required
                .as_array()
                .ok_or_else(|| fail("Invalid required fields", 400))?;
            let names: Option<BTreeSet<&str>> = required.iter().map(Value::as_str).collect();
            require(
                names.as_ref().is_some_and(|names| {
                    names.len() == required.len()
                        && names.iter().all(|name| properties.contains_key(*name))
                }),
                "Invalid required fields",
            )?;
            let regex =
                regex::Regex::new(r"^[A-Za-z_][A-Za-z0-9_]{0,39}$").expect("constant regex");
            for (key, value) in properties {
                require(regex.is_match(key), "Invalid parameter name")?;
                schema_check(value, depth + 1)?;
            }
        }
        "array" => {
            allowed.extend(["items", "maxItems", "minItems"]);
            let max = uint(s.get("maxItems"))
                .filter(|v| *v <= 16)
                .ok_or_else(|| fail("Arrays require maxItems <= 16", 400))?;
            let min = uint(s.get("minItems")).unwrap_or(0);
            require(
                s.get("minItems")
                    .is_none_or(|_| uint(s.get("minItems")).is_some())
                    && min <= max,
                "Invalid array minimum",
            )?;
            schema_check(
                s.get("items")
                    .ok_or_else(|| fail("Invalid parameter schema", 400))?,
                depth + 1,
            )?;
        }
        "string" => {
            allowed.extend(["maxLength", "minLength"]);
            let max = uint(s.get("maxLength"))
                .filter(|v| (1..=256).contains(v))
                .ok_or_else(|| fail("Strings require maxLength <= 256", 400))?;
            let min = uint(s.get("minLength")).unwrap_or(0);
            require(
                s.get("minLength")
                    .is_none_or(|_| uint(s.get("minLength")).is_some())
                    && min <= max,
                "Invalid string minimum",
            )?;
        }
        "number" | "integer" => {
            allowed.extend(["minimum", "maximum"]);
            let min = s
                .get("minimum")
                .and_then(finite)
                .ok_or_else(|| fail("Numbers require finite bounds", 400))?;
            let max = s
                .get("maximum")
                .and_then(finite)
                .ok_or_else(|| fail("Numbers require finite bounds", 400))?;
            require(min <= max, "Reversed numeric bounds")?;
        }
        "boolean" => {}
        _ => return Err(fail("Unsupported schema type", 400)),
    }
    require(
        s.keys().all(|key| allowed.contains(key.as_str())),
        "Unsupported schema keyword",
    )?;
    require(
        s.get("description")
            .is_none_or(|value| value.as_str().is_some_and(|text| length(text) <= 300)),
        "Invalid schema description",
    )?;
    if let Some(values) = s.get("enum") {
        let values = values
            .as_array()
            .filter(|values| (1..=32).contains(&values.len()))
            .ok_or_else(|| fail("Invalid enum", 400))?;
        let mut base = s.clone();
        base.remove("enum");
        for value in values {
            validate(value, &Value::Object(base.clone()))?;
        }
    }
    Ok(())
}
pub fn validate(value: &Value, schema: &Value) -> Result<()> {
    let s = object(schema)?;
    match s.get("type").and_then(Value::as_str).unwrap_or("") {
        "object" => {
            let properties = s
                .get("properties")
                .and_then(Value::as_object)
                .ok_or_else(|| fail("Invalid parameter schema", 400))?;
            let values = value
                .as_object()
                .ok_or_else(|| fail("Unexpected or missing arguments", 400))?;
            require(
                values.keys().all(|key| properties.contains_key(key))
                    && s.get("required")
                        .and_then(Value::as_array)
                        .is_none_or(|required| {
                            required
                                .iter()
                                .all(|key| key.as_str().is_some_and(|key| values.contains_key(key)))
                        }),
                "Unexpected or missing arguments",
            )?;
            for (key, value) in values {
                validate(value, &properties[key])?;
            }
        }
        "array" => {
            let values = value
                .as_array()
                .ok_or_else(|| fail("Invalid array length", 400))?;
            require(
                values.len() >= uint(s.get("minItems")).unwrap_or(0) as usize
                    && values.len() <= uint(s.get("maxItems")).unwrap_or(0) as usize,
                "Invalid array length",
            )?;
            for value in values {
                validate(
                    value,
                    s.get("items")
                        .ok_or_else(|| fail("Invalid parameter schema", 400))?,
                )?;
            }
        }
        "string" => {
            require(
                value.as_str().is_some_and(|value| {
                    length(value) >= uint(s.get("minLength")).unwrap_or(0) as usize
                        && length(value) <= uint(s.get("maxLength")).unwrap_or(0) as usize
                }),
                "Invalid string",
            )?;
        }
        "boolean" => require(value.is_boolean(), "Expected boolean")?,
        "number" | "integer" => {
            let number = finite(value).ok_or_else(|| fail("Number outside allowed bounds", 400))?;
            require(
                (s["type"] != "integer" || value.is_i64() || value.is_u64())
                    && number >= s.get("minimum").and_then(finite).unwrap_or(f64::INFINITY)
                    && number
                        <= s.get("maximum")
                            .and_then(finite)
                            .unwrap_or(f64::NEG_INFINITY),
                "Number outside allowed bounds",
            )?;
        }
        _ => return Err(fail("Unsupported schema type", 400)),
    }
    if let Some(values) = s.get("enum").and_then(Value::as_array) {
        require(
            values.iter().any(|choice| choice == value),
            "Value is not an allowed choice",
        )?;
    }
    Ok(())
}
pub fn capabilities(value: &Value) -> Result<Vec<Capability>> {
    bounded(value, 14000, "Capabilities")?;
    let raw = value
        .as_array()
        .filter(|value| (1..=12).contains(&value.len()))
        .ok_or_else(|| fail("Provide 1–12 capabilities", 400))?;
    let mut names = BTreeSet::new();
    let name_re = regex::Regex::new(r"^[a-z][a-z0-9_]{0,39}$").expect("constant regex");
    let mut result = Vec::new();
    for value in raw {
        let c: Capability = serde_json::from_value(value.clone())
            .map_err(|_| fail("Invalid capability fields", 400))?;
        require(
            name_re.is_match(&c.name) && names.insert(c.name.clone()),
            "Invalid or duplicate capability",
        )?;
        require(
            (1..=300).contains(&length(&c.description)),
            "Invalid capability description",
        )?;
        require(
            c.max_duration_s.is_finite() && c.max_duration_s > 0. && c.max_duration_s <= 30.,
            "Capability duration must be <= 30 seconds",
        )?;
        schema_check(&c.parameters, 0)?;
        require(
            c.parameters["type"] == "object",
            "Capability parameters must be an object",
        )?;
        result.push(c);
    }
    Ok(result)
}
pub fn digest(value: &Value) -> Result<String> {
    capabilities(value)?;
    Ok(format!(
        "{:x}",
        Sha256::digest(json_string(value, true).as_bytes())
    ))
}
pub fn robot_id(value: &str) -> Result<&str> {
    require(
        regex::Regex::new(r"^[A-Za-z0-9][A-Za-z0-9_-]{0,79}$")
            .expect("constant regex")
            .is_match(value),
        "Invalid robot_id",
    )?;
    Ok(value)
}
pub fn proposal(value: &Value, caps: &Value) -> Result<Proposal> {
    bounded(value, 12000, "Plan")?;
    let plan: Proposal =
        serde_json::from_value(value.clone()).map_err(|_| fail("Invalid plan fields", 400))?;
    require(length(&plan.summary) <= 1000, "Invalid plan summary")?;
    require(length(&plan.question) <= 1000, "Invalid clarification")?;
    require(plan.steps.len() <= 6, "Maximum 6 steps")?;
    require(
        (plan.needs_clarification && !plan.question.trim().is_empty() && plan.steps.is_empty())
            || (!plan.needs_clarification && !plan.steps.is_empty()),
        "A plan needs steps, or an explicit clarification question",
    )?;
    let capabilities: BTreeMap<_, _> = capabilities(caps)?
        .into_iter()
        .map(|c| (c.name.clone(), c))
        .collect();
    let mut total = 0.;
    for step in &plan.steps {
        let capability = capabilities
            .get(&step.capability)
            .ok_or_else(|| fail("Unknown capability", 400))?;
        validate(&step.arguments, &capability.parameters)?;
        require(
            step.duration_s.is_finite()
                && step.duration_s > 0.
                && step.duration_s <= capability.max_duration_s,
            "Invalid step duration",
        )?;
        total += step.duration_s;
    }
    require(total <= 60., "Plan duration exceeds 60 seconds")?;
    Ok(plan)
}
pub fn envelope(value: &Value, caps: &Value, expected_robot: &str, now: f64) -> Result<Proposal> {
    let m: Mission =
        serde_json::from_value(value.clone()).map_err(|_| fail("Invalid mission", 400))?;
    require(m.protocol_version == VERSION, "Unsupported protocol")?;
    require(valid_id(&m.id), "Invalid mission ID")?;
    require(
        m.robot_id == robot_id(expected_robot)? && m.capabilities_digest == digest(caps)?,
        "Robot or capability mismatch",
    )?;
    require(
        [m.issued_at, m.expires_at, m.observed_at, now]
            .iter()
            .all(|v| v.is_finite()),
        "Invalid mission timestamp",
    )?;
    require(
        m.observed_at >= now - 120.
            && m.observed_at <= now + 5.
            && m.observed_at <= m.issued_at + 5.
            && m.issued_at >= now - 120.
            && m.issued_at <= now + 5.
            && now < m.expires_at
            && m.expires_at <= (m.issued_at + 120.).min(m.observed_at + 120.),
        "Expired or stale mission",
    )?;
    require(m.requires_local_approval, "Local approval is required")?;
    proposal(
        value
            .get("plan")
            .ok_or_else(|| fail("Invalid plan fields", 400))?,
        caps,
    )
}
