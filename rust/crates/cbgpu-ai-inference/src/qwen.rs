//! Adapt the pinned local llama-server to the reservation scheduler's per-job protocol.
use crate::{native::Result, protocol, worker::atomic};
use reqwest::{blocking::Client, redirect::Policy};
use serde_json::{Value, json};
use std::{
    io::{BufRead, BufReader},
    os::unix::process::CommandExt,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

pub const MODEL_ID: &str = "cbgpu-qwen38";
const CONTEXT: &str = "65536";
const MODEL: &str = "Qwen3.8-27B-UD-IQ4_XS.gguf";
const PROJECTOR: &str = "mmproj-F16.gguf";
const MAX_OUTPUT: u64 = 4096;

struct Server(Child);
impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn configured_path(variable: &str) -> Result<PathBuf> {
    std::env::var_os(variable)
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .and_then(|path| path.canonicalize().ok())
        .ok_or_else(|| "Qwen3.8 runtime is not configured".into())
}

fn payload(input: &Value) -> Result<Value> {
    let messages = input["messages"].as_array().ok_or("Use chat messages")?;
    let output = input["max_tokens"].as_u64().unwrap_or(1024);
    if messages.is_empty() || messages.len() > 256 || !(1..=MAX_OUTPUT).contains(&output) {
        return Err("Qwen3.8 request exceeds the model limits".into());
    }
    for message in messages {
        if let Some(parts) = message["content"].as_array() {
            if message["role"] != "user" || parts.is_empty() || parts.len() > 16 {
                return Err("Invalid multipart chat message".into());
            }
            for part in parts {
                match part["type"].as_str() {
                    Some("text") if part["text"].is_string() => {}
                    Some("image_url")
                        if part["image_url"]["url"].as_str().is_some_and(|url| {
                            [
                                "data:image/png;base64,",
                                "data:image/jpeg;base64,",
                                "data:image/webp;base64,",
                            ]
                            .iter()
                            .any(|prefix| url.starts_with(prefix))
                        }) => {}
                    _ => return Err("Only embedded PNG, JPEG and WebP images are accepted".into()),
                }
            }
        }
    }
    let mut body = json!({"messages":messages,"max_tokens":output,"stream":true,
        "stream_options":{"include_usage":true},"temperature":0.2});
    if let Some(tools) = function_tools(input) {
        // Reuse the validated JSON tool protocol. This model's bundled template stalls
        // when llama-server compiles its native tool grammar.
        let (mut prepared, _) = protocol::prepare(messages, tools, &input["tool_choice"]);
        for (original, message) in messages.iter().zip(prepared.iter_mut().skip(1)) {
            if original["content"].is_array() {
                message["content"] = original["content"].clone();
            }
        }
        body["messages"] = json!(prepared);
        body["response_format"] = json!({"type":"json_object"});
    }
    Ok(body)
}

fn function_tools(input: &Value) -> Option<&[Value]> {
    input["tools"]
        .as_array()
        .map(Vec::as_slice)
        .filter(|tools| !tools.is_empty() && input["tool_choice"] != "none")
}

fn start_server(socket: &Path) -> Result<Server> {
    let runtime = configured_path("CBGPU_AI_QWEN38_RUNTIME")?;
    let assets = configured_path("CBGPU_AI_QWEN38_ASSETS")?;
    let assigned = std::env::var("CUDA_VISIBLE_DEVICES").unwrap_or_default();
    let approved = std::env::var("CBGPU_AI_QWEN38_GPU_UUID").unwrap_or_default();
    let index = std::env::var("CBGPU_AI_QWEN38_VULKAN_INDEX").unwrap_or_default();
    if assigned != approved
        || !assigned.starts_with("GPU-")
        || uuid::Uuid::parse_str(&assigned[4..]).is_err()
        || !index.parse::<u8>().is_ok_and(|n| n.to_string() == index)
    {
        return Err("Qwen3.8 requires its scheduler-assigned GPU".into());
    }
    let mut command = Command::new(runtime.join("llama-server"));
    command
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("CUDA_VISIBLE_DEVICES", "-1")
        .env("GGML_VK_VISIBLE_DEVICES", index)
        .env(
            "GGML_BACKEND_PATH",
            runtime.join("vulkan/libggml-vulkan.so"),
        )
        .env("LD_LIBRARY_PATH", &runtime)
        .args(["--device", "Vulkan0", "--host"])
        .arg(socket)
        .arg("--model")
        .arg(assets.join(MODEL))
        .arg("--mmproj")
        .arg(assets.join(PROJECTOR))
        .args([
            "--mmproj-device",
            "none",
            "--gpu-layers",
            "99",
            "--ctx-size",
            CONTEXT,
            "--parallel",
            "1",
            "--flash-attn",
            "on",
            "--cache-type-k",
            "q8_0",
            "--cache-type-v",
            "q4_0",
            "--threads",
            "8",
            "--threads-batch",
            "8",
            "--chat-template-kwargs",
            "{\"enable_thinking\":false}",
            "--offline",
            "--no-ui",
            "--no-context-shift",
            "--log-verbosity",
            "1",
        ])
        .stdin(Stdio::null());
    let parent = std::process::id();
    // The legacy scheduler and native scheduler kill this worker's process group on preemption.
    // Also terminate the child if the worker unexpectedly disappears.
    unsafe {
        command.pre_exec(move || {
            if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            if libc::getppid() as u32 != parent {
                return Err(std::io::Error::from_raw_os_error(libc::ESRCH));
            }
            Ok(())
        });
    }
    command
        .spawn()
        .map(Server)
        .map_err(|_| "Qwen3.8 worker could not start".into())
}

pub fn chat(folder: &Path, input: &Value) -> Result<Value> {
    let body = payload(input)?;
    let directory = tempfile::Builder::new()
        .prefix("cbgpu-qwen-")
        .tempdir_in("/tmp")
        .map_err(|_| "Cannot create private inference socket")?;
    let socket = directory.path().join("engine.sock");
    atomic(
        &folder.join("progress.json"),
        &json!({"message":"Loading Qwen3.8"}),
    )?;
    let mut server = start_server(&socket)?;
    let client = Client::builder()
        .unix_socket(socket)
        .no_proxy()
        .redirect(Policy::none())
        .connect_timeout(Duration::from_secs(1))
        .timeout(Duration::from_secs(660))
        .build()
        .map_err(|_| "Cannot initialize private inference client")?;
    let deadline = Instant::now() + Duration::from_secs(75);
    loop {
        if server
            .0
            .try_wait()
            .map_err(|_| "Cannot check Qwen3.8 worker")?
            .is_some()
            || Instant::now() >= deadline
        {
            return Err("Qwen3.8 failed to load on the assigned GPU".into());
        }
        if client
            .get("http://localhost/health")
            .timeout(Duration::from_secs(1))
            .send()
            .is_ok_and(|response| response.status().is_success())
        {
            break;
        }
        thread::sleep(Duration::from_millis(200));
    }
    atomic(
        &folder.join("progress.json"),
        &json!({"message":"Processing the conversation with Qwen3.8"}),
    )?;
    let response = client
        .post("http://localhost/v1/chat/completions")
        .json(&body)
        .send()
        .map_err(|_| "Qwen3.8 inference was interrupted")?;
    if !response.status().is_success() {
        return Err(format!(
            "Qwen3.8 rejected the request (HTTP {})",
            response.status().as_u16()
        ));
    }
    completion(BufReader::new(response), input, |text| {
        atomic(&folder.join("partial.json"), &json!({"text":text}))
    })
}

fn completion(
    reader: impl BufRead,
    input: &Value,
    mut partial: impl FnMut(&str) -> Result<()>,
) -> Result<Value> {
    let mut text = String::new();
    let mut usage = Value::Null;
    let mut finish = None;
    let tools = function_tools(input);
    let mut flushed = Instant::now() - Duration::from_secs(1);
    let mut done = false;
    let mut base = json!({});
    for line in reader.lines() {
        let line = line.map_err(|_| "Qwen3.8 output was interrupted")?;
        let Some(data) = line.strip_prefix("data: ") else {
            continue;
        };
        if data == "[DONE]" {
            done = true;
            break;
        }
        let chunk: Value = serde_json::from_str(data).map_err(|_| "Invalid Qwen3.8 output")?;
        if chunk.get("error").is_some() {
            return Err("Qwen3.8 could not complete this request".into());
        }
        if chunk["usage"].is_object() {
            usage = chunk["usage"].clone();
        }
        if let Some(reason) = chunk["choices"][0]["finish_reason"].as_str() {
            finish = Some(reason.to_owned());
        }
        if chunk["id"].is_string() {
            base["id"] = chunk["id"].clone();
        }
        if chunk["created"].is_number() {
            base["created"] = chunk["created"].clone();
        }
        let delta = &chunk["choices"][0]["delta"];
        if let Some(content) = delta["content"].as_str() {
            text.push_str(content);
            if text.len() > 400_000 {
                return Err("Qwen3.8 output is too large".into());
            }
            if tools.is_none() && flushed.elapsed() >= Duration::from_millis(80) {
                partial(&text)?;
                flushed = Instant::now();
            }
        }
    }
    let finish = finish
        .filter(|_| done)
        .ok_or("Qwen3.8 output did not finish")?;
    let (message, finish) = if let Some(tools) = tools {
        let (message, reason) = protocol::decode(&text, tools, &input["tool_choice"])?;
        (message, reason.to_owned())
    } else {
        if !text.is_empty() {
            partial(&text)?;
        }
        (json!({"role":"assistant","content":text}), finish)
    };
    base["object"] = json!("chat.completion");
    base["model"] = json!(MODEL_ID);
    base["choices"] = json!([{"index":0,"message":message,"finish_reason":finish}]);
    base["usage"] = usage;
    Ok(base)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_tool_calls_before_returning_them() {
        let chunk = json!({"choices":[{"delta":{"content":"{\"type\":\"tool\",\"name\":\"weather\",\"arguments\":{}}"},"finish_reason":"stop"}],
            "usage":{"total_tokens":24}});
        let stream = format!("data: {chunk}\n\ndata: [DONE]\n\n");
        let input = json!({"tool_choice":"required","tools":[{"function":{"name":"weather"}}]});
        let result = completion(stream.as_bytes(), &input, |_| {
            panic!("Do not stream raw tool JSON")
        })
        .unwrap();
        assert_eq!(result["choices"][0]["finish_reason"], "tool_calls");
        assert_eq!(
            result["choices"][0]["message"]["tool_calls"][0]["function"]["name"],
            "weather"
        );
        assert_eq!(result["usage"]["total_tokens"], 24);
        assert!(
            completion(
                stream.as_bytes(),
                &json!({"tools":[{"function":{"name":"other"}}]}),
                |_| Ok(())
            )
            .is_err()
        );
        assert!(
            completion(
                stream.split("data: [DONE]").next().unwrap().as_bytes(),
                &input,
                |_| Ok(())
            )
            .is_err()
        );
    }

    #[test]
    fn permits_embedded_images_only() {
        let mut input = json!({"messages":[{"role":"user","content":[{"type":"image_url","image_url":{"url":"data:image/png;base64,AA=="}}]}]});
        assert!(payload(&input).is_ok());
        input["messages"][0]["content"][0]["image_url"]["url"] = json!("http://localhost/private");
        assert!(payload(&input).is_err());
    }
}
