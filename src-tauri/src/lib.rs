use base64::{engine::general_purpose, Engine as _};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    env, fs,
    io::Write,
    path::{Component, Path, PathBuf},
    process::{Command, Output, Stdio},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tauri::{AppHandle, Emitter, Manager};

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "lowercase")]
struct ChatMessage {
    role: String,
    content: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ChatCompletionRequest {
    provider_name: String,
    base_url: String,
    api_key: String,
    model: String,
    reasoning_effort: Option<String>,
    temperature: Option<f32>,
    system_prompt: Option<String>,
    workspace_path: Option<String>,
    code_tools_enabled: Option<bool>,
    stream_id: Option<String>,
    messages: Vec<ChatMessage>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ChatCompletionResponse {
    content: String,
    trace_steps: Vec<ChatTraceStep>,
    usage: Option<ChatCompletionUsage>,
}

#[derive(Debug, Serialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
struct ChatCompletionUsage {
    prompt_tokens: Option<u64>,
    completion_tokens: Option<u64>,
    total_tokens: Option<u64>,
    prompt_cache_hit_tokens: Option<u64>,
    prompt_cache_miss_tokens: Option<u64>,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct ChatTraceStep {
    kind: String,
    text: String,
    detail: Option<String>,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct ChatCompletionStreamEvent {
    stream_id: String,
    event_type: String,
    trace_kind: Option<String>,
    text: String,
    detail: Option<String>,
    usage: Option<ChatCompletionUsage>,
}

#[derive(Debug, Default)]
struct ToolCallAccumulator {
    id: String,
    call_type: String,
    function_name: String,
    function_arguments: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ApplyPatchRequest {
    workspace_path: String,
    patch_text: String,
    files: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ApplyPatchResponse {
    applied_files: Vec<String>,
    stdout: String,
    stderr: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct InspectCodeWorkspaceRequest {
    workspace_path: String,
    query: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct InspectCodeWorkspaceResponse {
    tool: String,
    content: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SaveAppCacheRequest {
    cache_directory: Option<String>,
    settings: Value,
    member_library: Value,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AppCacheState {
    default_cache_directory: String,
    cache_directory: String,
    settings: Option<Value>,
    member_library: Option<Value>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AvatarCacheResponse {
    path: String,
}

const MAX_CHAT_COMPLETION_TURNS: usize = 8;
const MAX_CODE_TOOL_ROUNDS: usize = 8;
const MAX_CHAT_TRACE_STEPS: usize = 160;
const TRACE_STEP_TEXT_LIMIT: usize = 280;
const TOOL_TRACE_DETAIL_LIMIT: usize = 6000;
const CHAT_COMPLETION_STREAM_EVENT: &str = "chat-completion-stream";
const FINAL_ANSWER_INSTRUCTION: &str = "The code reading tool budget for this response is exhausted. Use the tool results already provided and write the final answer now. Do not request more tool calls.";
const CACHE_LOCATION_FILE: &str = "cache-location.json";
const SETTINGS_FILE: &str = "settings.json";
const MEMBER_LIBRARY_FILE: &str = "member-library.json";
const AVATAR_DIR: &str = "avatars";

fn app_data_dir(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_data_dir()
        .map_err(|error| format!("Failed to resolve app data directory: {}", error))
}

fn read_json_file(path: &Path) -> Result<Option<Value>, String> {
    if !path.exists() {
        return Ok(None);
    }

    let content = fs::read_to_string(path)
        .map_err(|error| format!("Failed to read {:?}: {}", path, error))?;

    serde_json::from_str(&content)
        .map(Some)
        .map_err(|error| format!("Failed to parse {:?}: {}", path, error))
}

fn write_json_file(path: &Path, value: &Value) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("Failed to create {:?}: {}", parent, error))?;
    }

    let content = serde_json::to_string_pretty(value)
        .map_err(|error| format!("Failed to serialize {:?}: {}", path, error))?;

    fs::write(path, content).map_err(|error| format!("Failed to write {:?}: {}", path, error))
}

fn read_cache_directory(app: &AppHandle) -> Result<PathBuf, String> {
    let default_dir = app_data_dir(app)?;
    let location_path = default_dir.join(CACHE_LOCATION_FILE);

    let Some(config) = read_json_file(&location_path)? else {
        return Ok(default_dir);
    };

    let configured = config
        .get("cacheDirectory")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());

    Ok(configured.map(PathBuf::from).unwrap_or(default_dir))
}

fn write_cache_directory(app: &AppHandle, cache_directory: &Path) -> Result<(), String> {
    let default_dir = app_data_dir(app)?;
    fs::create_dir_all(&default_dir)
        .map_err(|error| format!("Failed to create {:?}: {}", default_dir, error))?;
    write_json_file(
        &default_dir.join(CACHE_LOCATION_FILE),
        &json!({ "cacheDirectory": cache_directory.to_string_lossy() }),
    )
}

fn ensure_cache_tree(cache_directory: &Path) -> Result<(), String> {
    fs::create_dir_all(cache_directory)
        .map_err(|error| format!("Failed to create {:?}: {}", cache_directory, error))?;
    fs::create_dir_all(cache_directory.join(AVATAR_DIR)).map_err(|error| {
        format!(
            "Failed to create {:?}: {}",
            cache_directory.join(AVATAR_DIR),
            error
        )
    })
}

#[tauri::command]
async fn load_app_cache(app: AppHandle) -> Result<AppCacheState, String> {
    let default_cache_directory = app_data_dir(&app)?;
    let cache_directory = read_cache_directory(&app)?;
    ensure_cache_tree(&cache_directory)?;

    Ok(AppCacheState {
        default_cache_directory: default_cache_directory.to_string_lossy().to_string(),
        cache_directory: cache_directory.to_string_lossy().to_string(),
        settings: read_json_file(&cache_directory.join(SETTINGS_FILE))?,
        member_library: read_json_file(&cache_directory.join(MEMBER_LIBRARY_FILE))?,
    })
}

#[tauri::command]
async fn save_app_cache(app: AppHandle, mut request: SaveAppCacheRequest) -> Result<(), String> {
    let cache_directory = request
        .cache_directory
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or(read_cache_directory(&app)?);

    ensure_cache_tree(&cache_directory)?;
    write_cache_directory(&app, &cache_directory)?;
    materialize_avatar_data_urls(&mut request.settings, &cache_directory)?;
    materialize_avatar_data_urls(&mut request.member_library, &cache_directory)?;
    write_json_file(&cache_directory.join(SETTINGS_FILE), &request.settings)?;
    write_json_file(
        &cache_directory.join(MEMBER_LIBRARY_FILE),
        &request.member_library,
    )?;
    Ok(())
}

fn avatar_extension_from_mime(mime_type: &str) -> &str {
    match mime_type {
        "image/jpeg" => "jpg",
        "image/svg+xml" => "svg",
        "image/webp" => "webp",
        "image/gif" => "gif",
        "image/bmp" => "bmp",
        _ => "png",
    }
}

fn write_avatar_data_url(value: &str, cache_directory: &Path) -> Result<Option<String>, String> {
    let Some(rest) = value.strip_prefix("data:") else {
        return Ok(None);
    };
    let Some((mime_type, encoded)) = rest.split_once(";base64,") else {
        return Ok(None);
    };

    if !mime_type.starts_with("image/") {
        return Ok(None);
    }

    let bytes = general_purpose::STANDARD
        .decode(encoded)
        .map_err(|error| format!("Failed to read local file list: {}", error))?;
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| format!("Failed to create avatar name: {}", error))?
        .as_nanos();
    let extension = avatar_extension_from_mime(mime_type);
    let target = cache_directory
        .join(AVATAR_DIR)
        .join(format!("avatar-{}.{}", stamp, extension));

    fs::write(&target, bytes)
        .map_err(|error| format!("Failed to write cached avatar {:?}: {}", target, error))?;
    Ok(Some(target.to_string_lossy().to_string()))
}

fn materialize_avatar_data_urls(value: &mut Value, cache_directory: &Path) -> Result<(), String> {
    match value {
        Value::Object(map) => {
            for (key, child) in map.iter_mut() {
                if key == "avatar" {
                    if let Some(avatar) = child.as_str() {
                        if let Some(path) = write_avatar_data_url(avatar, cache_directory)? {
                            *child = Value::String(path);
                            continue;
                        }
                    }
                }

                materialize_avatar_data_urls(child, cache_directory)?;
            }
        }
        Value::Array(items) => {
            for item in items {
                materialize_avatar_data_urls(item, cache_directory)?;
            }
        }
        _ => {}
    }

    Ok(())
}

fn is_supported_avatar(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|value| value.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some("png" | "jpg" | "jpeg" | "webp" | "gif" | "bmp" | "svg")
    )
}

#[tauri::command]
async fn copy_avatar_to_cache(
    app: AppHandle,
    source_path: String,
) -> Result<AvatarCacheResponse, String> {
    let source = PathBuf::from(source_path.trim());

    if !source.is_file() {
        return Err("Avatar source file does not exist.".to_string());
    }

    if !is_supported_avatar(&source) {
        return Err("Unsupported avatar image type.".to_string());
    }

    let cache_directory = read_cache_directory(&app)?;
    let avatar_directory = cache_directory.join(AVATAR_DIR);
    ensure_cache_tree(&cache_directory)?;

    let extension = source
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("png");
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| format!("Failed to create avatar name: {}", error))?
        .as_nanos();
    let target = avatar_directory.join(format!("avatar-{}.{}", stamp, extension));

    fs::copy(&source, &target)
        .map_err(|error| format!("Failed to read local file list: {}", error))?;

    Ok(AvatarCacheResponse {
        path: target.to_string_lossy().to_string(),
    })
}

#[tauri::command]
async fn chat_completion(
    app: AppHandle,
    request: ChatCompletionRequest,
) -> Result<ChatCompletionResponse, String> {
    if request.api_key.trim().is_empty() {
        return Err(format!(
            "{} API Key is not configured.",
            request.provider_name
        ));
    }

    if request.model.trim().is_empty() {
        return Err("Model name cannot be empty.".to_string());
    }

    let code_workspace = if request.code_tools_enabled.unwrap_or(false) {
        Some(validate_workspace(
            request.workspace_path.as_deref().unwrap_or(""),
        )?)
    } else {
        None
    };

    let mut messages: Vec<Value> = Vec::new();
    if let Some(system_prompt) = request.system_prompt.as_deref() {
        let trimmed = system_prompt.trim();
        if !trimmed.is_empty() {
            messages.push(json!({
                "role": "system",
                "content": trimmed,
            }));
        }
    }

    for message in &request.messages {
        messages.push(json!({
            "role": message.role,
            "content": message.content,
        }));
    }

    let endpoint = format!(
        "{}/chat/completions",
        request.base_url.trim().trim_end_matches('/')
    );
    let client = reqwest::Client::new();
    let is_deepseek = is_deepseek_provider(&request.provider_name, &request.base_url);
    let mut code_tool_called = false;
    let mut code_tool_rounds = 0usize;
    let mut final_answer_requested = false;
    let mut last_finish_reason: Option<String> = None;
    let mut last_usage: Option<ChatCompletionUsage> = None;
    let mut trace_steps: Vec<ChatTraceStep> = Vec::new();
    let stream_id = request
        .stream_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);

    for _ in 0..MAX_CHAT_COMPLETION_TURNS {
        let payload_messages = chat_payload_messages(&messages, final_answer_requested);
        let mut payload = json!({
            "model": request.model,
            "messages": payload_messages,
            "temperature": request.temperature.unwrap_or(0.7),
        });
        let strict_tool_schema = is_deepseek && code_workspace.is_some() && !final_answer_requested;

        apply_reasoning_payload(
            &mut payload,
            is_deepseek,
            request.reasoning_effort.as_deref(),
        );

        if code_workspace.is_some() && !final_answer_requested {
            payload["tools"] = code_tools_schema(is_deepseek);
            payload["tool_choice"] = if is_deepseek && !code_tool_called {
                json!("required")
            } else {
                json!("auto")
            };
        }

        let parsed = match send_chat_completion_request_maybe_stream(
            &app,
            stream_id.as_deref(),
            &client,
            &endpoint,
            &request.api_key,
            &request.provider_name,
            &payload,
        )
        .await
        {
            Ok(parsed) => parsed,
            Err(error) if strict_tool_schema => {
                payload["tools"] = code_tools_schema(false);
                payload["tool_choice"] = json!("auto");
                send_chat_completion_request_maybe_stream(
                    &app,
                    stream_id.as_deref(),
                    &client,
                    &endpoint,
                    &request.api_key,
                    &request.provider_name,
                    &payload,
                )
                .await
                .map_err(|fallback_error| {
                    format!(
                        "{}; fallback without strict tool schema also failed: {}",
                        error, fallback_error
                    )
                })?
            }
            Err(error) => return Err(error),
        };
        if let Some(usage) = usage_from_response(&parsed) {
            last_usage = Some(usage);
        }
        last_finish_reason = first_choice_finish_reason(&parsed);
        let message = parsed
            .get("choices")
            .and_then(|choices| choices.get(0))
            .and_then(|choice| choice.get("message"))
            .cloned()
            .ok_or_else(|| format!("{} returned no message.", request.provider_name))?;
        append_reasoning_trace_steps(&mut trace_steps, &message);

        if let (Some(workspace), Some(tool_calls)) = (
            code_workspace.as_ref(),
            message.get("tool_calls").and_then(Value::as_array),
        ) {
            if !tool_calls.is_empty() {
                messages.push(message.clone());
                for tool_call in tool_calls {
                    let call_step = tool_call_trace_step(tool_call);
                    emit_trace_step(&app, stream_id.as_deref(), &call_step);
                    append_trace_steps(&mut trace_steps, vec![call_step]);
                    let tool_result = execute_code_tool_call(workspace, tool_call);
                    let result_step = tool_result_trace_step(tool_call, &tool_result);
                    emit_trace_step(&app, stream_id.as_deref(), &result_step);
                    append_trace_steps(&mut trace_steps, vec![result_step]);
                    messages.push(tool_result);
                }
                code_tool_called = true;
                code_tool_rounds += 1;
                if code_tool_rounds >= MAX_CODE_TOOL_ROUNDS {
                    final_answer_requested = true;
                }
                continue;
            }
        }

        let content = message_content_text(&message);

        if !content.is_empty() {
            return Ok(ChatCompletionResponse {
                content,
                trace_steps,
                usage: last_usage,
            });
        }

        if code_tool_called && !final_answer_requested {
            final_answer_requested = true;
            continue;
        }
    }

    let reason = last_finish_reason
        .map(|value| format!(" finish_reason={}", value))
        .unwrap_or_default();
    Err(format!(
        "{} returned no displayable content.{}",
        request.provider_name, reason
    ))
}

fn is_deepseek_provider(provider_name: &str, base_url: &str) -> bool {
    provider_name.to_ascii_lowercase().contains("deepseek")
        || base_url.to_ascii_lowercase().contains("deepseek")
}

fn apply_reasoning_payload(payload: &mut Value, is_deepseek: bool, reasoning_effort: Option<&str>) {
    let trimmed = reasoning_effort.unwrap_or("").trim();
    let reasoning_enabled = !trimmed.is_empty() && trimmed != "off";

    if is_deepseek {
        payload["thinking"] = if reasoning_enabled {
            json!({ "type": "enabled" })
        } else {
            json!({ "type": "disabled" })
        };
    }

    if reasoning_enabled {
        payload["reasoning_effort"] = json!(trimmed);
    }
}

fn chat_payload_messages(messages: &[Value], final_answer_requested: bool) -> Vec<Value> {
    let mut payload_messages = messages.to_vec();

    if final_answer_requested {
        payload_messages.push(json!({
            "role": "user",
            "content": FINAL_ANSWER_INSTRUCTION,
        }));
    }

    payload_messages
}

fn first_choice_finish_reason(parsed: &Value) -> Option<String> {
    parsed
        .get("choices")
        .and_then(|choices| choices.get(0))
        .and_then(|choice| choice.get("finish_reason"))
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn message_content_text(message: &Value) -> String {
    match message.get("content") {
        Some(Value::String(content)) => content.trim().to_string(),
        Some(Value::Array(parts)) => parts
            .iter()
            .filter_map(|part| part.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n")
            .trim()
            .to_string(),
        _ => String::new(),
    }
}

fn value_text(value: &Value) -> String {
    match value {
        Value::String(content) => content.trim().to_string(),
        Value::Array(parts) => parts
            .iter()
            .filter_map(|part| {
                part.get("text")
                    .or_else(|| part.get("content"))
                    .and_then(Value::as_str)
            })
            .collect::<Vec<_>>()
            .join("\n")
            .trim()
            .to_string(),
        _ => String::new(),
    }
}

fn message_reasoning_text(message: &Value) -> String {
    for key in ["reasoning_content", "reasoning"] {
        if let Some(value) = message.get(key) {
            let text = value_text(value);

            if !text.is_empty() {
                return text;
            }
        }
    }

    String::new()
}

fn trace_step(kind: &str, text: String) -> ChatTraceStep {
    ChatTraceStep {
        kind: kind.to_string(),
        text,
        detail: None,
    }
}

fn trace_step_with_detail(kind: &str, text: String, detail: String) -> ChatTraceStep {
    ChatTraceStep {
        kind: kind.to_string(),
        text,
        detail: Some(truncate_text(detail, TOOL_TRACE_DETAIL_LIMIT)),
    }
}

fn usage_from_response(parsed: &Value) -> Option<ChatCompletionUsage> {
    let usage = parsed.get("usage")?;

    Some(ChatCompletionUsage {
        prompt_tokens: usage.get("prompt_tokens").and_then(Value::as_u64),
        completion_tokens: usage.get("completion_tokens").and_then(Value::as_u64),
        total_tokens: usage.get("total_tokens").and_then(Value::as_u64),
        prompt_cache_hit_tokens: usage.get("prompt_cache_hit_tokens").and_then(Value::as_u64),
        prompt_cache_miss_tokens: usage
            .get("prompt_cache_miss_tokens")
            .and_then(Value::as_u64),
    })
    .filter(|usage| {
        usage.prompt_tokens.is_some()
            || usage.completion_tokens.is_some()
            || usage.total_tokens.is_some()
            || usage.prompt_cache_hit_tokens.is_some()
            || usage.prompt_cache_miss_tokens.is_some()
    })
}

fn split_trace_text(text: &str) -> Vec<String> {
    let mut steps = Vec::new();

    for raw_line in text.lines() {
        let line = raw_line.trim();

        if line.is_empty() {
            continue;
        }

        let chars: Vec<char> = line.chars().collect();

        if chars.len() <= TRACE_STEP_TEXT_LIMIT {
            steps.push(line.to_string());
            continue;
        }

        for chunk in chars.chunks(TRACE_STEP_TEXT_LIMIT) {
            steps.push(chunk.iter().collect());
        }
    }

    steps
}

fn append_trace_steps(trace_steps: &mut Vec<ChatTraceStep>, next_steps: Vec<ChatTraceStep>) {
    for step in next_steps {
        if trace_steps.len() >= MAX_CHAT_TRACE_STEPS {
            return;
        }

        if !step.text.trim().is_empty() {
            trace_steps.push(step);
        }
    }
}

fn append_reasoning_trace_steps(trace_steps: &mut Vec<ChatTraceStep>, message: &Value) {
    let reasoning = message_reasoning_text(message);
    let steps = split_trace_text(&reasoning)
        .into_iter()
        .map(|line| trace_step("reasoning", line))
        .collect();

    append_trace_steps(trace_steps, steps);
}

fn parsed_tool_arguments(function: &Value) -> Value {
    let arguments = function
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!("{}"));

    if let Some(arguments_text) = arguments.as_str() {
        serde_json::from_str::<Value>(arguments_text)
            .unwrap_or_else(|_| Value::String(arguments_text.to_string()))
    } else {
        arguments
    }
}

fn compact_trace_json(value: &Value) -> String {
    serde_json::to_string(value)
        .map(|text| truncate_text(text, TRACE_STEP_TEXT_LIMIT))
        .unwrap_or_else(|_| "<unreadable arguments>".to_string())
}

fn pretty_trace_json(value: &Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_else(|_| "<unreadable arguments>".to_string())
}

fn tool_call_trace_step(tool_call: &Value) -> ChatTraceStep {
    let function = tool_call.get("function").unwrap_or(&Value::Null);
    let name = function
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("unknown_tool");
    let arguments = parsed_tool_arguments(function);
    let text = if name == "codegraph_explore" {
        let query = arguments
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let max_files = arguments.get("maxFiles").and_then(Value::as_u64);

        match max_files {
            Some(max_files) => format!(
                "codegraph_explore query=\"{}\" maxFiles={}",
                truncate_text(query.to_string(), 180),
                max_files
            ),
            None => format!(
                "codegraph_explore query=\"{}\"",
                truncate_text(query.to_string(), 200)
            ),
        }
    } else {
        format!("{} {}", name, compact_trace_json(&arguments))
    };

    let detail = format!(
        "Tool: {}\nArguments:\n{}",
        name,
        pretty_trace_json(&arguments)
    );

    trace_step_with_detail("tool", text, detail)
}

fn tool_result_trace_step(tool_call: &Value, tool_message: &Value) -> ChatTraceStep {
    let function = tool_call.get("function").unwrap_or(&Value::Null);
    let name = function
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("unknown_tool");
    let content = message_content_text(tool_message);
    let route = if content.contains("Local command fallback") || content.contains("local fallback")
    {
        "local fallback"
    } else if content.contains("CodeGraph") {
        "CodeGraph"
    } else {
        "tool"
    };
    let first_line = content
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("");
    let text = if first_line.is_empty() {
        format!("{} returned an empty result", name)
    } else {
        format!(
            "{} returned {} chars via {}: {}",
            name,
            content.chars().count(),
            route,
            truncate_text(first_line.to_string(), 160)
        )
    };

    let detail = format!(
        "Tool: {}\nResult characters: {}\n\n{}",
        name,
        content.chars().count(),
        content
    );

    trace_step_with_detail("tool", text, detail)
}

fn finalize_tool_function(mut function: Value, strict: bool) -> Value {
    if strict {
        function["strict"] = json!(true);
        function["parameters"]["additionalProperties"] = json!(false);
    }

    json!({
        "type": "function",
        "function": function
    })
}

fn code_tools_schema(strict: bool) -> Value {
    json!([
        finalize_tool_function(
            json!({
                "name": "codegraph_explore",
                "description": "Read the current workspace with CodeGraph for symbols, responsibilities, and call paths. The `Found N symbols across M files` line is query-scoped, not the total index file count.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "query": {
                            "type": "string",
                            "description": "The symbols, files, call flow, or implementation question to inspect."
                        },
                        "maxFiles": {
                            "type": "integer",
                            "description": "Optional maximum number of files to include source from. Defaults to 12 and is capped at 24.",
                            "minimum": 1,
                            "maximum": 24
                        }
                    },
                    "required": ["query"]
                }
            }),
            strict
        ),
        finalize_tool_function(
            json!({
                "name": "read_file",
                "description": "Read exact file contents from the workspace with line numbers. Use this when CodeGraph output omits implementation details.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "file": {
                            "type": "string",
                            "description": "Workspace-relative file path."
                        },
                        "startLine": {
                            "type": "integer",
                            "description": "1-based line to start from. Defaults to 1.",
                            "minimum": 1
                        },
                        "maxLines": {
                            "type": "integer",
                            "description": "Maximum lines to read. Defaults to 240 and is capped at 1000.",
                            "minimum": 1,
                            "maximum": 1000
                        }
                    },
                    "required": ["file"]
                }
            }),
            strict
        ),
        finalize_tool_function(
            json!({
                "name": "list_files",
                "description": "List files under the workspace or a subdirectory. Use this to discover nearby files before reading them.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "Optional workspace-relative directory path."
                        },
                        "recursive": {
                            "type": "boolean",
                            "description": "Whether to recurse. Defaults to true."
                        },
                        "maxResults": {
                            "type": "integer",
                            "description": "Maximum files to return. Defaults to 120 and is capped at 500.",
                            "minimum": 1,
                            "maximum": 500
                        }
                    },
                    "required": []
                }
            }),
            strict
        ),
        finalize_tool_function(
            json!({
                "name": "search_files",
                "description": "Search text in workspace files with ripgrep-style output. Use for finding identifiers, errors, strings, and TODOs.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "query": {
                            "type": "string",
                            "description": "Regex or literal text to search for."
                        },
                        "path": {
                            "type": "string",
                            "description": "Optional workspace-relative path to search in."
                        },
                        "caseSensitive": {
                            "type": "boolean",
                            "description": "Defaults to false."
                        },
                        "literal": {
                            "type": "boolean",
                            "description": "Treat query as fixed text instead of regex. Defaults to false."
                        },
                        "maxResults": {
                            "type": "integer",
                            "description": "Maximum matches to return. Defaults to 80 and is capped at 300.",
                            "minimum": 1,
                            "maximum": 300
                        }
                    },
                    "required": ["query"]
                }
            }),
            strict
        ),
        finalize_tool_function(
            json!({
                "name": "glob_files",
                "description": "Find files by glob pattern, for example `src/**/*.vue` or `**/*.rs`.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "pattern": {
                            "type": "string",
                            "description": "Glob pattern relative to the workspace."
                        },
                        "path": {
                            "type": "string",
                            "description": "Optional workspace-relative directory to search from."
                        },
                        "maxResults": {
                            "type": "integer",
                            "description": "Maximum files to return. Defaults to 120 and is capped at 500.",
                            "minimum": 1,
                            "maximum": 500
                        }
                    },
                    "required": ["pattern"]
                }
            }),
            strict
        ),
        finalize_tool_function(
            json!({
                "name": "write_file",
                "description": "Create, overwrite, or append to a UTF-8 text file inside the workspace.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "file": {
                            "type": "string",
                            "description": "Workspace-relative file path."
                        },
                        "content": {
                            "type": "string",
                            "description": "Text content to write."
                        },
                        "mode": {
                            "type": "string",
                            "enum": ["overwrite", "create", "append"],
                            "description": "Write mode. Defaults to overwrite."
                        },
                        "createParents": {
                            "type": "boolean",
                            "description": "Whether to create missing parent directories. Defaults to true."
                        }
                    },
                    "required": ["file", "content"]
                }
            }),
            strict
        ),
        finalize_tool_function(
            json!({
                "name": "create_directory",
                "description": "Create a directory inside the workspace, including missing parents.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "Workspace-relative directory path."
                        }
                    },
                    "required": ["path"]
                }
            }),
            strict
        ),
        finalize_tool_function(
            json!({
                "name": "delete_path",
                "description": "Delete a workspace-relative file or, with recursive=true, a directory. Refuses workspace root and sensitive/generated paths.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "Workspace-relative path to delete."
                        },
                        "recursive": {
                            "type": "boolean",
                            "description": "Required for deleting directories. Defaults to false."
                        }
                    },
                    "required": ["path"]
                }
            }),
            strict
        ),
        finalize_tool_function(
            json!({
                "name": "move_path",
                "description": "Move or rename a file or directory inside the workspace.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "from": {
                            "type": "string",
                            "description": "Existing workspace-relative source path."
                        },
                        "to": {
                            "type": "string",
                            "description": "Workspace-relative destination path."
                        },
                        "createParents": {
                            "type": "boolean",
                            "description": "Whether to create missing destination parent directories. Defaults to true."
                        }
                    },
                    "required": ["from", "to"]
                }
            }),
            strict
        ),
        finalize_tool_function(
            json!({
                "name": "apply_patch",
                "description": "Apply a unified diff patch inside the workspace. Use checkOnly=true to validate without changing files.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "patchText": {
                            "type": "string",
                            "description": "Unified diff text accepted by git apply."
                        },
                        "checkOnly": {
                            "type": "boolean",
                            "description": "Validate only without applying. Defaults to false."
                        }
                    },
                    "required": ["patchText"]
                }
            }),
            strict
        ),
        finalize_tool_function(
            json!({
                "name": "run_command",
                "description": "Run a non-interactive command in the workspace, such as tests, formatters, or git diff. Prefer command plus args instead of shell syntax.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "command": {
                            "type": "string",
                            "description": "Executable name, for example npm, cargo, git, rg, node, or python."
                        },
                        "args": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "Command arguments."
                        },
                        "cwd": {
                            "type": "string",
                            "description": "Optional workspace-relative working directory."
                        },
                        "timeoutMs": {
                            "type": "integer",
                            "description": "Timeout in milliseconds. Defaults to 30000 and is capped at 120000.",
                            "minimum": 1000,
                            "maximum": 120000
                        }
                    },
                    "required": ["command"]
                }
            }),
            strict
        )
    ])
}

async fn send_chat_completion_request(
    client: &reqwest::Client,
    endpoint: &str,
    api_key: &str,
    provider_name: &str,
    payload: &Value,
) -> Result<Value, String> {
    let response = client
        .post(endpoint)
        .bearer_auth(api_key.trim())
        .json(payload)
        .send()
        .await
        .map_err(|error| format!("Request to {} failed: {}", provider_name, error))?;

    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|error| format!("Failed to read {} response: {}", provider_name, error))?;

    if !status.is_success() {
        return Err(format!(
            "{} returned HTTP {}: {}",
            provider_name, status, body
        ));
    }

    serde_json::from_str(&body)
        .map_err(|error| format!("Failed to parse {} response: {}", provider_name, error))
}

async fn send_chat_completion_request_maybe_stream(
    app: &AppHandle,
    stream_id: Option<&str>,
    client: &reqwest::Client,
    endpoint: &str,
    api_key: &str,
    provider_name: &str,
    payload: &Value,
) -> Result<Value, String> {
    if let Some(stream_id) = stream_id {
        send_chat_completion_stream_request(
            app,
            stream_id,
            client,
            endpoint,
            api_key,
            provider_name,
            payload,
        )
        .await
    } else {
        send_chat_completion_request(client, endpoint, api_key, provider_name, payload).await
    }
}

fn emit_stream_event(
    app: &AppHandle,
    stream_id: Option<&str>,
    event_type: &str,
    trace_kind: Option<&str>,
    text: impl Into<String>,
    detail: Option<String>,
    usage: Option<ChatCompletionUsage>,
) {
    let Some(stream_id) = stream_id else {
        return;
    };

    let _ = app.emit(
        CHAT_COMPLETION_STREAM_EVENT,
        ChatCompletionStreamEvent {
            stream_id: stream_id.to_string(),
            event_type: event_type.to_string(),
            trace_kind: trace_kind.map(str::to_string),
            text: text.into(),
            detail,
            usage,
        },
    );
}

fn emit_trace_step(app: &AppHandle, stream_id: Option<&str>, step: &ChatTraceStep) {
    emit_stream_event(
        app,
        stream_id,
        "traceStep",
        Some(&step.kind),
        step.text.clone(),
        step.detail.clone(),
        None,
    );
}

fn emit_trace_chunk(app: &AppHandle, stream_id: &str, trace_kind: &str, text: &str) {
    emit_stream_event(
        app,
        Some(stream_id),
        "traceChunk",
        Some(trace_kind),
        text,
        None,
        None,
    );
}

fn emit_content_chunk(app: &AppHandle, stream_id: &str, text: &str) {
    emit_stream_event(app, Some(stream_id), "contentChunk", None, text, None, None);
}

fn emit_usage_event(app: &AppHandle, stream_id: &str, usage: ChatCompletionUsage) {
    emit_stream_event(app, Some(stream_id), "usage", None, "", None, Some(usage));
}

fn sse_event_separator(buffer: &str) -> Option<(usize, usize)> {
    match (buffer.find("\n\n"), buffer.find("\r\n\r\n")) {
        (Some(lf), Some(crlf)) if crlf < lf => Some((crlf, 4)),
        (Some(lf), _) => Some((lf, 2)),
        (_, Some(crlf)) => Some((crlf, 4)),
        _ => None,
    }
}

fn sse_data_lines(event_block: &str) -> Vec<String> {
    event_block
        .lines()
        .filter_map(|line| {
            let line = line.trim_end_matches('\r');
            line.strip_prefix("data:")
                .map(|data| data.trim_start().to_string())
        })
        .collect()
}

fn ensure_tool_call_slot(tool_calls: &mut Vec<ToolCallAccumulator>, index: usize) {
    while tool_calls.len() <= index {
        tool_calls.push(ToolCallAccumulator::default());
    }
}

fn append_delta_tool_calls(delta_tool_calls: &[Value], tool_calls: &mut Vec<ToolCallAccumulator>) {
    for delta_call in delta_tool_calls {
        let index = delta_call
            .get("index")
            .and_then(Value::as_u64)
            .unwrap_or(tool_calls.len() as u64) as usize;
        ensure_tool_call_slot(tool_calls, index);
        let accumulator = &mut tool_calls[index];

        if let Some(id) = delta_call.get("id").and_then(Value::as_str) {
            accumulator.id.push_str(id);
        }

        if let Some(call_type) = delta_call.get("type").and_then(Value::as_str) {
            accumulator.call_type.push_str(call_type);
        }

        if let Some(function) = delta_call.get("function") {
            if let Some(name) = function.get("name").and_then(Value::as_str) {
                accumulator.function_name.push_str(name);
            }

            if let Some(arguments) = function.get("arguments").and_then(Value::as_str) {
                accumulator.function_arguments.push_str(arguments);
            }
        }
    }
}

fn tool_call_accumulators_to_values(tool_calls: Vec<ToolCallAccumulator>) -> Vec<Value> {
    tool_calls
        .into_iter()
        .enumerate()
        .filter(|(_, call)| {
            !call.function_name.trim().is_empty() || !call.function_arguments.trim().is_empty()
        })
        .map(|(index, call)| {
            json!({
                "id": if call.id.is_empty() {
                    format!("streamed-tool-call-{}", index)
                } else {
                    call.id
                },
                "type": if call.call_type.is_empty() {
                    "function".to_string()
                } else {
                    call.call_type
                },
                "function": {
                    "name": call.function_name,
                    "arguments": call.function_arguments,
                },
            })
        })
        .collect()
}

fn apply_stream_delta(
    app: &AppHandle,
    stream_id: &str,
    parsed: &Value,
    content: &mut String,
    reasoning: &mut String,
    tool_calls: &mut Vec<ToolCallAccumulator>,
    finish_reason: &mut Option<String>,
    usage: &mut Option<ChatCompletionUsage>,
) {
    if let Some(next_usage) = usage_from_response(parsed) {
        emit_usage_event(app, stream_id, next_usage.clone());
        *usage = Some(next_usage);
    }

    let Some(choices) = parsed.get("choices").and_then(Value::as_array) else {
        return;
    };

    for choice in choices {
        if let Some(reason) = choice.get("finish_reason").and_then(Value::as_str) {
            *finish_reason = Some(reason.to_string());
        }

        let Some(delta) = choice.get("delta") else {
            continue;
        };

        let reasoning_chunk = ["reasoning_content", "reasoning"]
            .into_iter()
            .filter_map(|key| delta.get(key).and_then(Value::as_str))
            .find(|chunk| !chunk.is_empty());

        if let Some(chunk) = reasoning_chunk {
            reasoning.push_str(chunk);
            emit_trace_chunk(app, stream_id, "reasoning", chunk);
        }

        if let Some(chunk) = delta.get("content").and_then(Value::as_str) {
            if !chunk.is_empty() {
                content.push_str(chunk);
                emit_content_chunk(app, stream_id, chunk);
            }
        }

        if let Some(delta_tool_calls) = delta.get("tool_calls").and_then(Value::as_array) {
            append_delta_tool_calls(delta_tool_calls, tool_calls);
        }
    }
}

async fn send_chat_completion_stream_request(
    app: &AppHandle,
    stream_id: &str,
    client: &reqwest::Client,
    endpoint: &str,
    api_key: &str,
    provider_name: &str,
    payload: &Value,
) -> Result<Value, String> {
    let mut payload = payload.clone();
    payload["stream"] = json!(true);
    payload["stream_options"] = json!({ "include_usage": true });

    let response = client
        .post(endpoint)
        .bearer_auth(api_key.trim())
        .json(&payload)
        .send()
        .await
        .map_err(|error| format!("Request to {} failed: {}", provider_name, error))?;

    let status = response.status();

    if !status.is_success() {
        let body = response
            .text()
            .await
            .map_err(|error| format!("Failed to read {} response: {}", provider_name, error))?;

        return Err(format!(
            "{} returned HTTP {}: {}",
            provider_name, status, body
        ));
    }

    let mut content = String::new();
    let mut reasoning = String::new();
    let mut tool_calls: Vec<ToolCallAccumulator> = Vec::new();
    let mut finish_reason: Option<String> = None;
    let mut usage: Option<ChatCompletionUsage> = None;
    let mut buffer = String::new();
    let mut stream = response.bytes_stream();

    while let Some(chunk) = stream.next().await {
        let bytes =
            chunk.map_err(|error| format!("Failed to read {} stream: {}", provider_name, error))?;
        buffer.push_str(&String::from_utf8_lossy(&bytes));

        while let Some((separator_index, separator_len)) = sse_event_separator(&buffer) {
            let event_block = buffer[..separator_index].to_string();
            buffer = buffer[(separator_index + separator_len)..].to_string();

            for data in sse_data_lines(&event_block) {
                if data == "[DONE]" {
                    break;
                }

                let parsed: Value = serde_json::from_str(&data).map_err(|error| {
                    format!("Failed to parse {} stream event: {}", provider_name, error)
                })?;
                apply_stream_delta(
                    app,
                    stream_id,
                    &parsed,
                    &mut content,
                    &mut reasoning,
                    &mut tool_calls,
                    &mut finish_reason,
                    &mut usage,
                );
            }
        }
    }

    if !buffer.trim().is_empty() {
        for data in sse_data_lines(&buffer) {
            if data == "[DONE]" {
                continue;
            }

            let parsed: Value = serde_json::from_str(&data).map_err(|error| {
                format!(
                    "Failed to parse {} final stream event: {}",
                    provider_name, error
                )
            })?;
            apply_stream_delta(
                app,
                stream_id,
                &parsed,
                &mut content,
                &mut reasoning,
                &mut tool_calls,
                &mut finish_reason,
                &mut usage,
            );
        }
    }

    let mut message = json!({
        "role": "assistant",
        "content": content,
    });

    if !reasoning.trim().is_empty() {
        message["reasoning_content"] = json!(reasoning);
    }

    let tool_calls = tool_call_accumulators_to_values(tool_calls);

    if !tool_calls.is_empty() {
        message["tool_calls"] = Value::Array(tool_calls);
    }

    Ok(json!({
        "choices": [
            {
                "message": message,
                "finish_reason": finish_reason,
            }
        ],
        "usage": usage
    }))
}

fn execute_code_tool_call(workspace: &Path, tool_call: &Value) -> Value {
    let tool_call_id = tool_call
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or("codegraph-tool-call");
    let function = tool_call.get("function").unwrap_or(&Value::Null);
    let name = function.get("name").and_then(Value::as_str).unwrap_or("");
    let arguments = parsed_tool_arguments(function);

    let content = match name {
        "codegraph_explore" => execute_codegraph_explore_tool(workspace, &arguments),
        "read_file" => read_workspace_file_tool(workspace, &arguments),
        "list_files" => list_workspace_files_tool(workspace, &arguments),
        "search_files" => search_workspace_files_tool(workspace, &arguments),
        "glob_files" => glob_workspace_files_tool(workspace, &arguments),
        "write_file" => write_workspace_file_tool(workspace, &arguments),
        "create_directory" => create_workspace_directory_tool(workspace, &arguments),
        "delete_path" => delete_workspace_path_tool(workspace, &arguments),
        "move_path" => move_workspace_path_tool(workspace, &arguments),
        "apply_patch" => apply_patch_tool(workspace, &arguments),
        "run_command" => run_workspace_command_tool(workspace, &arguments),
        _ => Err(format!("Unknown tool: {}", name)),
    };

    json!({
        "role": "tool",
        "tool_call_id": tool_call_id,
        "content": content.unwrap_or_else(|error| format!("Tool {} failed: {}", name, error)),
    })
}

fn execute_codegraph_explore_tool(workspace: &Path, arguments: &Value) -> Result<String, String> {
    let query = arguments
        .get("query")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let max_files = arguments.get("maxFiles").and_then(Value::as_u64);

    if query.trim().is_empty() {
        return Err("CodeGraph query cannot be empty.".to_string());
    }

    match run_codegraph_explore(workspace, query, max_files) {
        Ok(content) => Ok(content),
        Err(codegraph_error) => read_local_code_context(workspace, query)
            .map(|fallback| {
                format!(
                    "CodeGraph tool was called, but CodeGraph execution failed and local fallback was used.\nCodeGraph error: {}\n\n{}",
                    codegraph_error, fallback
                )
            })
            .map_err(|fallback_error| {
                format!(
                    "CodeGraph could not read this workspace: {}. Local fallback failed: {}",
                    codegraph_error, fallback_error
                )
            }),
    }
}

fn tool_arg_string<'a>(arguments: &'a Value, key: &str) -> &'a str {
    arguments
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("")
}

fn tool_arg_bool(arguments: &Value, key: &str, default: bool) -> bool {
    arguments
        .get(key)
        .and_then(Value::as_bool)
        .unwrap_or(default)
}

fn tool_arg_string_array(arguments: &Value, key: &str) -> Vec<String> {
    arguments
        .get(key)
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn tool_arg_usize(arguments: &Value, key: &str, default: usize, min: usize, max: usize) -> usize {
    arguments
        .get(key)
        .and_then(Value::as_u64)
        .map(|value| value as usize)
        .unwrap_or(default)
        .clamp(min, max)
}

fn resolve_workspace_relative_path(workspace: &Path, raw_path: &str) -> Result<PathBuf, String> {
    let normalized = raw_path.trim().replace('\\', "/");

    if normalized.is_empty() {
        return Ok(workspace.to_path_buf());
    }

    let relative = Path::new(&normalized);

    if relative.is_absolute() {
        return Err(format!("Absolute paths are not allowed: {}", normalized));
    }

    for component in relative.components() {
        if matches!(
            component,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        ) {
            return Err(format!("Path traversal is not allowed: {}", normalized));
        }
    }

    let target = fs::canonicalize(workspace.join(relative))
        .map_err(|error| format!("Path does not exist: {} ({})", normalized, error))?;

    if !target.starts_with(workspace) {
        return Err(format!("Path escapes workspace: {}", normalized));
    }

    Ok(target)
}

fn resolve_workspace_target_path(workspace: &Path, raw_path: &str) -> Result<PathBuf, String> {
    let normalized = validate_relative_file(raw_path)?;
    Ok(workspace.join(normalized))
}

fn ensure_target_stays_in_workspace(workspace: &Path, target: &Path) -> Result<(), String> {
    let existing = if target.exists() {
        target.to_path_buf()
    } else {
        target
            .parent()
            .map(Path::to_path_buf)
            .ok_or_else(|| "Target has no parent directory.".to_string())?
    };
    let canonical = fs::canonicalize(&existing)
        .map_err(|error| format!("Failed to verify {}: {}", existing.display(), error))?;

    if !canonical.starts_with(workspace) {
        return Err(format!("Path escapes workspace: {}", target.display()));
    }

    Ok(())
}

fn workspace_relative_display(workspace: &Path, path: &Path) -> String {
    path.strip_prefix(workspace)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn is_skipped_directory(path: &Path) -> bool {
    path.components().any(|component| {
        component
            .as_os_str()
            .to_str()
            .map(|part| matches!(part, ".git" | "node_modules" | "dist" | "target"))
            .unwrap_or(false)
    })
}

fn collect_workspace_files(
    workspace: &Path,
    directory: &Path,
    recursive: bool,
    files: &mut Vec<String>,
    max_results: usize,
) -> Result<(), String> {
    if files.len() >= max_results || is_skipped_directory(directory) {
        return Ok(());
    }

    let entries = fs::read_dir(directory)
        .map_err(|error| format!("Failed to list {}: {}", directory.display(), error))?;

    for entry in entries {
        if files.len() >= max_results {
            break;
        }

        let entry = entry.map_err(|error| error.to_string())?;
        let path = entry.path();

        if path.is_dir() {
            if recursive {
                collect_workspace_files(workspace, &path, recursive, files, max_results)?;
            }
            continue;
        }

        if path.is_file() {
            files.push(workspace_relative_display(workspace, &path));
        }
    }

    Ok(())
}

fn list_workspace_files_tool(workspace: &Path, arguments: &Value) -> Result<String, String> {
    let path = resolve_workspace_relative_path(workspace, tool_arg_string(arguments, "path"))?;
    let recursive = tool_arg_bool(arguments, "recursive", true);
    let max_results = tool_arg_usize(arguments, "maxResults", 120, 1, 500);

    if !path.is_dir() {
        return Err("list_files path must be a directory.".to_string());
    }

    let mut files = Vec::new();
    collect_workspace_files(workspace, &path, recursive, &mut files, max_results)?;
    files.sort();

    if files.is_empty() {
        return Ok("No files found.".to_string());
    }

    Ok(format!(
        "Files under {} (showing up to {}):\n{}",
        workspace_relative_display(workspace, &path),
        max_results,
        files.join("\n")
    ))
}

fn read_workspace_file_tool(workspace: &Path, arguments: &Value) -> Result<String, String> {
    let file = tool_arg_string(arguments, "file");

    if file.is_empty() {
        return Err("read_file requires a file path.".to_string());
    }

    let path = resolve_workspace_relative_path(workspace, file)?;

    if !path.is_file() {
        return Err(format!("Not a file: {}", file));
    }

    let metadata = fs::metadata(&path).map_err(|error| error.to_string())?;

    if metadata.len() > 5_000_000 {
        return Err("File is too large to read directly; use search_files first.".to_string());
    }

    let start_line = tool_arg_usize(arguments, "startLine", 1, 1, usize::MAX);
    let max_lines = tool_arg_usize(arguments, "maxLines", 240, 1, 1000);
    let content =
        fs::read_to_string(&path).map_err(|error| format!("Failed to read {}: {}", file, error))?;
    let lines: Vec<&str> = content.lines().collect();

    if start_line > lines.len().max(1) {
        return Ok(format!(
            "{} has {} lines; startLine {} is past the end.",
            workspace_relative_display(workspace, &path),
            lines.len(),
            start_line
        ));
    }

    let start_index = start_line.saturating_sub(1);
    let end_index = (start_index + max_lines).min(lines.len());
    let numbered = lines[start_index..end_index]
        .iter()
        .enumerate()
        .map(|(index, line)| format!("{}\t{}", start_index + index + 1, line))
        .collect::<Vec<_>>()
        .join("\n");

    Ok(format!(
        "{} lines {}-{} of {}:\n{}",
        workspace_relative_display(workspace, &path),
        start_line,
        end_index,
        lines.len(),
        truncate_text(numbered, 18_000)
    ))
}

fn run_rg_files(workspace: &Path, args: &[String]) -> Result<String, String> {
    let output = Command::new("rg")
        .current_dir(workspace)
        .args(args)
        .output()
        .map_err(|error| format!("Failed to run rg: {}", error))?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    if !output.status.success() && output.status.code() != Some(1) && stdout.trim().is_empty() {
        return Err(if stderr.trim().is_empty() {
            "rg returned no usable result.".to_string()
        } else {
            stderr.trim().to_string()
        });
    }

    Ok(stdout)
}

fn rg_exclude_args() -> Vec<String> {
    vec![
        "--glob".to_string(),
        "!node_modules".to_string(),
        "--glob".to_string(),
        "!dist".to_string(),
        "--glob".to_string(),
        "!target".to_string(),
        "--glob".to_string(),
        "!.git".to_string(),
    ]
}

fn search_workspace_files_tool(workspace: &Path, arguments: &Value) -> Result<String, String> {
    let query = tool_arg_string(arguments, "query");

    if query.is_empty() {
        return Err("search_files requires a query.".to_string());
    }

    let max_results = tool_arg_usize(arguments, "maxResults", 80, 1, 300);
    let mut args = vec![
        "--line-number".to_string(),
        "--no-heading".to_string(),
        "--color".to_string(),
        "never".to_string(),
    ];
    args.extend(rg_exclude_args());

    if !tool_arg_bool(arguments, "caseSensitive", false) {
        args.push("--ignore-case".to_string());
    }

    if tool_arg_bool(arguments, "literal", false) {
        args.push("--fixed-strings".to_string());
    }

    args.push(query.to_string());

    let path_arg = tool_arg_string(arguments, "path");
    if !path_arg.is_empty() {
        let path = resolve_workspace_relative_path(workspace, path_arg)?;
        args.push(workspace_relative_display(workspace, &path));
    }

    let stdout = run_rg_files(workspace, &args)?;
    let lines = stdout
        .lines()
        .filter(|line| !line.trim().is_empty())
        .take(max_results)
        .collect::<Vec<_>>();

    if lines.is_empty() {
        return Ok(format!("No matches for `{}`.", query));
    }

    Ok(format!(
        "Matches for `{}` (showing up to {}):\n{}",
        query,
        max_results,
        truncate_text(lines.join("\n"), 18_000)
    ))
}

fn glob_workspace_files_tool(workspace: &Path, arguments: &Value) -> Result<String, String> {
    let pattern = tool_arg_string(arguments, "pattern");

    if pattern.is_empty() {
        return Err("glob_files requires a pattern.".to_string());
    }

    let max_results = tool_arg_usize(arguments, "maxResults", 120, 1, 500);
    let mut args = vec!["--files".to_string()];
    args.extend(rg_exclude_args());
    args.push("--glob".to_string());
    args.push(pattern.to_string());

    let path_arg = tool_arg_string(arguments, "path");
    if !path_arg.is_empty() {
        let path = resolve_workspace_relative_path(workspace, path_arg)?;
        args.push(workspace_relative_display(workspace, &path));
    }

    let stdout = run_rg_files(workspace, &args)?;
    let files = stdout
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .take(max_results)
        .collect::<Vec<_>>();

    if files.is_empty() {
        return Ok(format!("No files matched `{}`.", pattern));
    }

    Ok(format!(
        "Files matching `{}` (showing up to {}):\n{}",
        pattern,
        max_results,
        files.join("\n")
    ))
}

fn ensure_parent_directory(path: &Path, create_parents: bool) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "Target path has no parent directory.".to_string())?;

    if parent.exists() {
        return Ok(());
    }

    if !create_parents {
        return Err(format!(
            "Parent directory does not exist: {}",
            parent.display()
        ));
    }

    fs::create_dir_all(parent)
        .map_err(|error| format!("Failed to create {}: {}", parent.display(), error))
}

fn write_workspace_file_tool(workspace: &Path, arguments: &Value) -> Result<String, String> {
    let file = tool_arg_string(arguments, "file");
    let content = arguments
        .get("content")
        .and_then(Value::as_str)
        .unwrap_or("");
    let mode = tool_arg_string(arguments, "mode");
    let create_parents = tool_arg_bool(arguments, "createParents", true);

    if file.is_empty() {
        return Err("write_file requires a file path.".to_string());
    }

    if content.len() > 2_000_000 {
        return Err("write_file content is too large.".to_string());
    }

    let path = resolve_workspace_target_path(workspace, file)?;
    ensure_parent_directory(&path, create_parents)?;
    ensure_target_stays_in_workspace(workspace, &path)?;

    match mode {
        "" | "overwrite" => {
            fs::write(&path, content)
                .map_err(|error| format!("Failed to write {}: {}", file, error))?;
        }
        "create" => {
            if path.exists() {
                return Err(format!("File already exists: {}", file));
            }
            fs::write(&path, content)
                .map_err(|error| format!("Failed to create {}: {}", file, error))?;
        }
        "append" => {
            let mut file_handle = fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                .map_err(|error| format!("Failed to open {}: {}", file, error))?;
            file_handle
                .write_all(content.as_bytes())
                .map_err(|error| format!("Failed to append {}: {}", file, error))?;
        }
        other => return Err(format!("Unsupported write_file mode: {}", other)),
    }

    Ok(format!(
        "{} {} ({} bytes).",
        match mode {
            "append" => "Appended",
            "create" => "Created",
            _ => "Wrote",
        },
        workspace_relative_display(workspace, &path),
        content.len()
    ))
}

fn create_workspace_directory_tool(workspace: &Path, arguments: &Value) -> Result<String, String> {
    let path_arg = tool_arg_string(arguments, "path");

    if path_arg.is_empty() {
        return Err("create_directory requires a path.".to_string());
    }

    let path = resolve_workspace_target_path(workspace, path_arg)?;
    fs::create_dir_all(&path)
        .map_err(|error| format!("Failed to create {}: {}", path_arg, error))?;
    ensure_target_stays_in_workspace(workspace, &path)?;

    Ok(format!(
        "Created directory {}.",
        workspace_relative_display(workspace, &path)
    ))
}

fn delete_workspace_path_tool(workspace: &Path, arguments: &Value) -> Result<String, String> {
    let path_arg = tool_arg_string(arguments, "path");
    let recursive = tool_arg_bool(arguments, "recursive", false);

    if path_arg.is_empty() {
        return Err("delete_path requires a path.".to_string());
    }

    let validated_path = validate_relative_file(path_arg)?;
    let path = resolve_workspace_relative_path(workspace, &validated_path)?;

    if path == workspace {
        return Err("delete_path refuses to delete the workspace root.".to_string());
    }

    if path.is_dir() {
        if !recursive {
            return Err("delete_path requires recursive=true for directories.".to_string());
        }
        fs::remove_dir_all(&path)
            .map_err(|error| format!("Failed to delete directory {}: {}", path_arg, error))?;
    } else {
        fs::remove_file(&path)
            .map_err(|error| format!("Failed to delete file {}: {}", path_arg, error))?;
    }

    Ok(format!("Deleted {}.", path_arg))
}

fn move_workspace_path_tool(workspace: &Path, arguments: &Value) -> Result<String, String> {
    let from = tool_arg_string(arguments, "from");
    let to = tool_arg_string(arguments, "to");
    let create_parents = tool_arg_bool(arguments, "createParents", true);

    if from.is_empty() || to.is_empty() {
        return Err("move_path requires both from and to.".to_string());
    }

    let validated_source = validate_relative_file(from)?;
    let source = resolve_workspace_relative_path(workspace, &validated_source)?;
    let target = resolve_workspace_target_path(workspace, to)?;

    if source == workspace {
        return Err("move_path refuses to move the workspace root.".to_string());
    }

    ensure_parent_directory(&target, create_parents)?;
    ensure_target_stays_in_workspace(workspace, &target)?;
    fs::rename(&source, &target)
        .map_err(|error| format!("Failed to move {} to {}: {}", from, to, error))?;

    Ok(format!("Moved {} to {}.", from, to))
}

fn apply_patch_tool(workspace: &Path, arguments: &Value) -> Result<String, String> {
    let patch_text = arguments
        .get("patchText")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    let check_only = tool_arg_bool(arguments, "checkOnly", false);

    if patch_text.is_empty() {
        return Err("apply_patch requires patchText.".to_string());
    }

    let request = ApplyPatchRequest {
        workspace_path: workspace.to_string_lossy().to_string(),
        patch_text: patch_text.to_string(),
        files: Vec::new(),
    };
    let applied_files = collect_patch_files(&request)?;
    let patch_file = write_temp_patch(patch_text)?;
    let check_result = run_git_apply(workspace, &patch_file, true);

    if let Err(error) = check_result {
        let _ = fs::remove_file(&patch_file);
        return Err(error);
    }

    if check_only {
        let _ = fs::remove_file(&patch_file);
        return Ok(format!(
            "Patch check passed for files:\n{}",
            applied_files.join("\n")
        ));
    }

    let (stdout, stderr) = run_git_apply(workspace, &patch_file, false)?;
    let _ = fs::remove_file(&patch_file);
    let output = [stdout.trim(), stderr.trim()]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("\n");

    Ok(format!(
        "Patch applied to files:\n{}\n{}",
        applied_files.join("\n"),
        if output.is_empty() {
            "git apply produced no output.".to_string()
        } else {
            truncate_text(output, 8_000)
        }
    ))
}

fn run_workspace_command_tool(workspace: &Path, arguments: &Value) -> Result<String, String> {
    let command = tool_arg_string(arguments, "command");
    let args = tool_arg_string_array(arguments, "args");
    let timeout_ms = tool_arg_usize(arguments, "timeoutMs", 30_000, 1_000, 120_000) as u64;
    let cwd_arg = tool_arg_string(arguments, "cwd");
    let cwd = if cwd_arg.is_empty() {
        workspace.to_path_buf()
    } else {
        resolve_workspace_relative_path(workspace, cwd_arg)?
    };

    if command.is_empty() {
        return Err("run_command requires a command.".to_string());
    }

    if command.contains('/') || command.contains('\\') {
        return Err("run_command command must be an executable name, not a path.".to_string());
    }

    if !cwd.is_dir() {
        return Err("run_command cwd must be a directory.".to_string());
    }

    let mut child = Command::new(command)
        .current_dir(&cwd)
        .args(&args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("Failed to start command: {}", error))?;

    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    loop {
        if child
            .try_wait()
            .map_err(|error| format!("Failed to poll command: {}", error))?
            .is_some()
        {
            break;
        }

        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!("Command timed out after {} ms.", timeout_ms));
        }

        std::thread::sleep(Duration::from_millis(50));
    }

    let output = child
        .wait_with_output()
        .map_err(|error| format!("Failed to read command output: {}", error))?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let code = output
        .status
        .code()
        .map(|value| value.to_string())
        .unwrap_or_else(|| "terminated".to_string());
    let combined = format!(
        "exit_code={}\nstdout:\n{}\nstderr:\n{}",
        code,
        stdout.trim(),
        stderr.trim()
    );

    if !output.status.success() {
        return Err(truncate_text(combined, 12_000));
    }

    Ok(truncate_text(combined, 12_000))
}

fn validate_workspace(workspace_path: &str) -> Result<PathBuf, String> {
    let trimmed = workspace_path.trim();

    if trimmed.is_empty() {
        return Err("Workspace path cannot be empty.".to_string());
    }

    let workspace = fs::canonicalize(trimmed).map_err(|error| {
        format!(
            "Workspace folder does not exist or is inaccessible: {}",
            error
        )
    })?;

    if !workspace.is_dir() {
        return Err("Workspace path must be a directory.".to_string());
    }

    Ok(workspace)
}

fn has_codegraph_index(workspace: &Path) -> bool {
    workspace
        .ancestors()
        .any(|path| path.join(".codegraph").is_dir())
}

fn codegraph_command_candidates() -> Vec<PathBuf> {
    let mut candidates = vec![PathBuf::from("codegraph"), PathBuf::from("codegraph.cmd")];

    if let Ok(app_data) = env::var("APPDATA") {
        candidates.push(PathBuf::from(app_data).join("npm").join("codegraph.cmd"));
    }

    if let Ok(user_profile) = env::var("USERPROFILE") {
        candidates.push(
            PathBuf::from(user_profile)
                .join("AppData")
                .join("Roaming")
                .join("npm")
                .join("codegraph.cmd"),
        );
    }

    candidates
}

fn run_script_command(script: &Path, workspace: &Path, args: &[&str]) -> Result<Output, String> {
    let extension = script
        .extension()
        .and_then(|value| value.to_str())
        .map(str::to_ascii_lowercase);

    if matches!(extension.as_deref(), Some("cmd" | "bat")) {
        let mut command = Command::new("cmd");
        command.current_dir(workspace).arg("/C").arg(script);
        for arg in args {
            command.arg(arg);
        }
        return command.output().map_err(|error| error.to_string());
    }

    let mut command = Command::new(script);
    command.current_dir(workspace);
    for arg in args {
        command.arg(arg);
    }
    command.output().map_err(|error| error.to_string())
}

fn run_codegraph_command(workspace: &Path, args: &[&str]) -> Result<Output, String> {
    let mut errors = Vec::new();

    for candidate in codegraph_command_candidates() {
        let output = run_script_command(&candidate, workspace, args);

        match output {
            Ok(output) => return Ok(output),
            Err(error) => errors.push(format!("{}: {}", candidate.display(), error)),
        }
    }

    Err(format!(
        "Failed to start CodeGraph. Tried: {}",
        errors.join("; ")
    ))
}

fn truncate_text(text: String, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text;
    }

    let mut truncated: String = text.chars().take(max_chars).collect();
    truncated.push_str("\n\n[Content truncated]");
    truncated
}

const DEFAULT_CODEGRAPH_MAX_FILES: u64 = 12;
const MAX_CODEGRAPH_MAX_FILES: u64 = 24;
const CODEGRAPH_EXPLORE_SCOPE_NOTE: &str = "CodeGraph explore note: `Found N symbols across M files` describes only this query's returned relevant symbols/files. It is not the total CodeGraph index file count, and should not be used as the index health/status summary. If a CodeGraph index status section is present, use that for status questions.";

fn normalize_codegraph_max_files(max_files: Option<u64>) -> u64 {
    max_files
        .unwrap_or(DEFAULT_CODEGRAPH_MAX_FILES)
        .clamp(1, MAX_CODEGRAPH_MAX_FILES)
}

fn contains_any(text: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| text.contains(needle))
}

fn is_codegraph_status_query(query: &str) -> bool {
    let lower = query.to_lowercase();

    contains_any(
        &lower,
        &[
            "codegraph status",
            "status",
            "health",
            "statistics",
            "stats",
            "coverage",
            "indexed files",
            "index coverage",
            "index statistics",
            "状态",
            "健康",
            "统计",
            "覆盖",
            "工作正常",
            "文件数",
        ],
    ) || (lower.contains("index")
        && contains_any(
            &lower,
            &[
                "up to date",
                "file count",
                "files indexed",
                "how many files",
                "total files",
            ],
        ))
        || (lower.contains("索引")
            && contains_any(
                &lower,
                &["状态", "统计", "覆盖", "文件", "正常", "健康", "多少"],
            ))
}

fn strip_ansi_escape_sequences(text: &str) -> String {
    let mut cleaned = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '\x1b' && matches!(chars.peek(), Some('[')) {
            chars.next();
            for next in chars.by_ref() {
                if ('@'..='~').contains(&next) {
                    break;
                }
            }
            continue;
        }

        cleaned.push(ch);
    }

    cleaned
}

fn run_codegraph_status(workspace: &Path) -> Result<String, String> {
    let output = run_codegraph_command(workspace, &["status"])?;
    let stdout = strip_ansi_escape_sequences(&String::from_utf8_lossy(&output.stdout));
    let stderr = strip_ansi_escape_sequences(&String::from_utf8_lossy(&output.stderr));

    if !output.status.success() || stdout.trim().is_empty() {
        return Err(if stderr.trim().is_empty() {
            "CodeGraph status returned no usable result.".to_string()
        } else {
            format!("CodeGraph status failed: {}", stderr.trim())
        });
    }

    Ok(stdout.trim().to_string())
}

fn format_codegraph_explore_output(explore_output: &str, status_output: Option<&str>) -> String {
    let mut sections = vec![CODEGRAPH_EXPLORE_SCOPE_NOTE.to_string()];

    if let Some(status) = status_output
        .map(str::trim)
        .filter(|status| !status.is_empty())
    {
        sections.push(format!("CodeGraph index status:\n{}", status));
    }

    sections.push(format!(
        "CodeGraph explore result:\n{}",
        explore_output.trim()
    ));
    sections.join("\n\n")
}

fn run_codegraph_explore(
    workspace: &Path,
    query: &str,
    max_files: Option<u64>,
) -> Result<String, String> {
    if !has_codegraph_index(workspace) {
        return Err(format!(
            "No .codegraph index was found for {}. Select an indexed workspace or run `codegraph init` and `codegraph index` in that project.",
            workspace.display()
        ));
    }

    let max_files_arg = normalize_codegraph_max_files(max_files).to_string();
    let output = run_codegraph_command(
        workspace,
        &["explore", "--max-files", max_files_arg.as_str(), query],
    )?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    if !output.status.success() || stdout.trim().is_empty() {
        return Err(if stderr.trim().is_empty() {
            "CodeGraph returned no usable result.".to_string()
        } else {
            format!("CodeGraph query failed: {}", stderr.trim())
        });
    }

    let status_output =
        if is_codegraph_status_query(query) {
            Some(run_codegraph_status(workspace).unwrap_or_else(|error| {
                format!("CodeGraph index status could not be read: {}", error)
            }))
        } else {
            None
        };

    Ok(truncate_text(
        format_codegraph_explore_output(&stdout, status_output.as_deref()),
        18_000,
    ))
}

fn is_code_file(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    matches!(
        Path::new(&lower)
            .extension()
            .and_then(|value| value.to_str()),
        Some(
            "ts" | "tsx"
                | "vue"
                | "js"
                | "jsx"
                | "rs"
                | "json"
                | "css"
                | "md"
                | "toml"
                | "yml"
                | "yaml"
                | "py"
                | "go"
                | "java"
                | "kt"
        )
    )
}

fn read_local_code_context(workspace: &Path, query: &str) -> Result<String, String> {
    let output = Command::new("rg")
        .current_dir(workspace)
        .arg("--files")
        .arg("-g")
        .arg("!node_modules")
        .arg("-g")
        .arg("!dist")
        .arg("-g")
        .arg("!target")
        .output()
        .or_else(|_| {
            Command::new("git")
                .current_dir(workspace)
                .arg("ls-files")
                .output()
        })
        .map_err(|error| format!("Failed to read local file list: {}", error))?;

    if !output.status.success() {
        return Err("Failed to read local file list.".to_string());
    }

    let files: Vec<String> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|line| line.trim().replace('\\', "/"))
        .filter(|line| !line.is_empty() && is_code_file(line))
        .take(240)
        .collect();

    let query_lower = query.to_ascii_lowercase();
    let mut selected: Vec<String> = files
        .iter()
        .filter(|file| query_lower.contains(&file.to_ascii_lowercase()))
        .take(8)
        .cloned()
        .collect();

    if selected.is_empty() {
        selected = files.iter().take(12).cloned().collect();
    }

    let mut sections = vec![
        "Local command fallback content follows. This is not a CodeGraph result and has no symbol graph analysis.".to_string(),
        "File list:".to_string(),
        files
            .iter()
            .take(80)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n"),
    ];

    for file in selected {
        let path = workspace.join(&file);
        if let Ok(metadata) = fs::metadata(&path) {
            if metadata.len() > 80_000 {
                continue;
            }
        }

        if let Ok(content) = fs::read_to_string(&path) {
            sections.push(format!(
                "\n--- {} ---\n{}",
                file,
                truncate_text(content, 4_000)
            ));
        }
    }

    Ok(truncate_text(sections.join("\n"), 18_000))
}

#[tauri::command]
async fn inspect_code_workspace(
    request: InspectCodeWorkspaceRequest,
) -> Result<InspectCodeWorkspaceResponse, String> {
    let workspace = validate_workspace(&request.workspace_path)?;
    let query = request.query.trim();

    if query.is_empty() {
        return Err("Code inspection query cannot be empty.".to_string());
    }

    match run_codegraph_explore(&workspace, query, None) {
        Ok(content) => Ok(InspectCodeWorkspaceResponse {
            tool: "CodeGraph".to_string(),
            content,
        }),
        Err(codegraph_error) => {
            let fallback = read_local_code_context(&workspace, query)?;
            Ok(InspectCodeWorkspaceResponse {
                tool: format!("LocalCommands (CodeGraph failed: {})", codegraph_error),
                content: fallback,
            })
        }
    }
}

fn validate_relative_file(file: &str) -> Result<String, String> {
    let normalized = file.trim().replace('\\', "/");

    if normalized.is_empty() {
        return Err("Patch contains an empty file path.".to_string());
    }

    let path = Path::new(&normalized);

    if path.is_absolute() {
        return Err(format!("Absolute paths are not allowed: {}", normalized));
    }

    for component in path.components() {
        match component {
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(format!("Path traversal is not allowed: {}", normalized));
            }
            Component::Normal(part) => {
                let text = part.to_string_lossy().to_ascii_lowercase();
                if matches!(
                    text.as_str(),
                    ".ssh" | ".aws" | ".config" | "node_modules" | "dist" | "target"
                ) || text.starts_with(".env")
                {
                    return Err(format!(
                        "Sensitive or generated paths are not allowed: {}",
                        normalized
                    ));
                }
            }
            _ => {}
        }
    }

    Ok(normalized)
}

fn collect_patch_files(request: &ApplyPatchRequest) -> Result<Vec<String>, String> {
    let mut files = request.files.clone();

    for line in request.patch_text.lines() {
        if let Some(path) = line.strip_prefix("+++ b/") {
            files.push(path.to_string());
        } else if let Some(path) = line.strip_prefix("--- a/") {
            files.push(path.to_string());
        } else if let Some(rest) = line.strip_prefix("diff --git a/") {
            if let Some((left, right)) = rest.split_once(" b/") {
                files.push(left.to_string());
                files.push(right.to_string());
            }
        }
    }

    let mut validated = Vec::new();
    for file in files {
        let file = validate_relative_file(&file)?;
        if !validated.contains(&file) {
            validated.push(file);
        }
    }

    if validated.is_empty() {
        return Err("No patch target files were detected.".to_string());
    }

    Ok(validated)
}

fn write_temp_patch(patch_text: &str) -> Result<PathBuf, String> {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| format!("Failed to create temporary patch name: {}", error))?
        .as_millis();
    let path = std::env::temp_dir().join(format!("matrixofprescience-{}.patch", stamp));
    let mut file = fs::File::create(&path)
        .map_err(|error| format!("Failed to create temporary patch: {}", error))?;

    file.write_all(patch_text.as_bytes())
        .map_err(|error| format!("Failed to write temporary patch: {}", error))?;

    Ok(path)
}

fn run_git_apply(
    workspace: &Path,
    patch_file: &Path,
    check_only: bool,
) -> Result<(String, String), String> {
    let mut command = Command::new("git");
    command
        .current_dir(workspace)
        .arg("apply")
        .arg("--whitespace=nowarn");

    if check_only {
        command.arg("--check");
    }

    let output = command
        .arg(patch_file)
        .output()
        .map_err(|error| format!("Failed to run git apply: {}", error))?;
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    if !output.status.success() {
        return Err(if stderr.trim().is_empty() {
            format!("git apply failed: {}", stdout)
        } else {
            format!("git apply failed: {}", stderr)
        });
    }

    Ok((stdout, stderr))
}

#[tauri::command]
async fn apply_patch_proposal(request: ApplyPatchRequest) -> Result<ApplyPatchResponse, String> {
    let workspace = validate_workspace(&request.workspace_path)?;
    let patch_text = request.patch_text.trim();

    if patch_text.is_empty() {
        return Err("Patch content cannot be empty.".to_string());
    }

    let applied_files = collect_patch_files(&request)?;
    let patch_file = write_temp_patch(patch_text)?;

    let result = (|| {
        run_git_apply(&workspace, &patch_file, true)?;
        let (stdout, stderr) = run_git_apply(&workspace, &patch_file, false)?;

        Ok(ApplyPatchResponse {
            applied_files,
            stdout,
            stderr,
        })
    })();

    let _ = fs::remove_file(patch_file);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_codegraph_max_files() {
        assert_eq!(
            normalize_codegraph_max_files(None),
            DEFAULT_CODEGRAPH_MAX_FILES
        );
        assert_eq!(normalize_codegraph_max_files(Some(0)), 1);
        assert_eq!(normalize_codegraph_max_files(Some(7)), 7);
        assert_eq!(
            normalize_codegraph_max_files(Some(99)),
            MAX_CODEGRAPH_MAX_FILES
        );
    }

    #[test]
    fn detects_codegraph_status_queries_without_matching_file_names() {
        assert!(is_codegraph_status_query("check CodeGraph status"));
        assert!(is_codegraph_status_query(
            "show index statistics and file count"
        ));
        assert!(!is_codegraph_status_query("open src/i18n/index.ts"));
    }

    #[test]
    fn strips_ansi_escape_sequences_from_status_output() {
        assert_eq!(
            strip_ansi_escape_sequences("\x1b[1mCodeGraph Status\x1b[0m\n\x1b[32m[OK]\x1b[0m"),
            "CodeGraph Status\n[OK]"
        );
    }

    #[test]
    fn formatted_codegraph_output_clarifies_query_scope() {
        let output = format_codegraph_explore_output(
            "Found 44 symbols across 3 files.\n\n**Source Code**",
            Some("Index Statistics:\n  Files:     26"),
        );

        assert!(output.contains("query's returned relevant symbols/files"));
        assert!(output.contains("not the total CodeGraph index file count"));
        assert!(output.contains("Files:     26"));
        assert!(output.contains("CodeGraph explore result:"));
    }

    #[test]
    fn deepseek_reasoning_payload_can_disable_thinking() {
        let mut payload = json!({});

        apply_reasoning_payload(&mut payload, true, Some("off"));

        assert_eq!(payload["thinking"], json!({ "type": "disabled" }));
        assert!(payload.get("reasoning_effort").is_none());
    }

    #[test]
    fn deepseek_reasoning_payload_enables_thinking_with_effort() {
        let mut payload = json!({});

        apply_reasoning_payload(&mut payload, true, Some("high"));

        assert_eq!(payload["thinking"], json!({ "type": "enabled" }));
        assert_eq!(payload["reasoning_effort"], json!("high"));
    }

    #[test]
    fn final_answer_request_appends_internal_instruction() {
        let messages = vec![json!({ "role": "user", "content": "question" })];
        let payload_messages = chat_payload_messages(&messages, true);

        assert_eq!(payload_messages.len(), 2);
        assert_eq!(payload_messages[0]["content"], json!("question"));
        assert_eq!(payload_messages[1]["role"], json!("user"));
        assert!(payload_messages[1]["content"]
            .as_str()
            .unwrap()
            .contains("final answer"));
    }

    #[test]
    fn extracts_message_content_from_string_and_text_parts() {
        assert_eq!(
            message_content_text(&json!({ "content": "  hello  " })),
            "hello"
        );
        assert_eq!(
            message_content_text(&json!({
                "content": [
                    { "type": "text", "text": "hello" },
                    { "type": "text", "text": "world" }
                ]
            })),
            "hello\nworld"
        );
    }

    #[test]
    fn extracts_deepseek_reasoning_content() {
        assert_eq!(
            message_reasoning_text(&json!({
                "reasoning_content": "  line one\nline two  ",
                "content": "answer"
            })),
            "line one\nline two"
        );
    }

    #[test]
    fn extracts_deepseek_prompt_cache_usage() {
        let usage = usage_from_response(&json!({
            "usage": {
                "prompt_tokens": 120,
                "completion_tokens": 30,
                "total_tokens": 150,
                "prompt_cache_hit_tokens": 90,
                "prompt_cache_miss_tokens": 30
            }
        }))
        .expect("usage should parse");

        assert_eq!(usage.prompt_tokens, Some(120));
        assert_eq!(usage.prompt_cache_hit_tokens, Some(90));
        assert_eq!(usage.prompt_cache_miss_tokens, Some(30));
    }

    #[test]
    fn code_tools_schema_includes_file_search_tools() {
        let schema = code_tools_schema(true);
        let names = schema
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|tool| tool.get("function")?.get("name")?.as_str())
            .collect::<Vec<_>>();

        assert!(names.contains(&"codegraph_explore"));
        assert!(names.contains(&"read_file"));
        assert!(names.contains(&"list_files"));
        assert!(names.contains(&"search_files"));
        assert!(names.contains(&"glob_files"));
        assert!(names.contains(&"write_file"));
        assert!(names.contains(&"create_directory"));
        assert!(names.contains(&"delete_path"));
        assert!(names.contains(&"move_path"));
        assert!(names.contains(&"apply_patch"));
        assert!(names.contains(&"run_command"));
    }

    #[test]
    fn read_file_tool_reads_line_ranges_and_blocks_traversal() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let workspace = std::env::temp_dir().join(format!("matrix-tool-test-{}", stamp));
        let src_dir = workspace.join("src");
        fs::create_dir_all(&src_dir).unwrap();
        fs::write(src_dir.join("main.rs"), "one\ntwo\nthree\n").unwrap();
        let workspace = fs::canonicalize(&workspace).unwrap();

        let content = read_workspace_file_tool(
            &workspace,
            &json!({ "file": "src/main.rs", "startLine": 2, "maxLines": 1 }),
        )
        .unwrap();

        assert!(content.contains("2\ttwo"));
        assert!(resolve_workspace_relative_path(&workspace, "../outside").is_err());

        let _ = fs::remove_dir_all(&workspace);
    }

    #[test]
    fn file_write_move_and_delete_tools_stay_in_workspace() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let workspace = std::env::temp_dir().join(format!("matrix-write-tool-test-{}", stamp));
        fs::create_dir_all(&workspace).unwrap();
        let workspace = fs::canonicalize(&workspace).unwrap();

        write_workspace_file_tool(
            &workspace,
            &json!({
                "file": "notes/one.txt",
                "content": "hello",
                "mode": "create"
            }),
        )
        .unwrap();
        assert!(workspace.join("notes/one.txt").is_file());

        move_workspace_path_tool(
            &workspace,
            &json!({
                "from": "notes/one.txt",
                "to": "notes/two.txt"
            }),
        )
        .unwrap();
        assert!(workspace.join("notes/two.txt").is_file());

        delete_workspace_path_tool(
            &workspace,
            &json!({
                "path": "notes/two.txt"
            }),
        )
        .unwrap();
        assert!(!workspace.join("notes/two.txt").exists());
        assert!(write_workspace_file_tool(
            &workspace,
            &json!({
                "file": "../outside.txt",
                "content": "no"
            }),
        )
        .is_err());

        let _ = fs::remove_dir_all(&workspace);
    }

    #[test]
    fn appends_reasoning_trace_lines() {
        let mut trace_steps = Vec::new();

        append_reasoning_trace_steps(
            &mut trace_steps,
            &json!({ "reasoning_content": "first\n\nsecond" }),
        );

        assert_eq!(trace_steps.len(), 2);
        assert_eq!(trace_steps[0].kind, "reasoning");
        assert_eq!(trace_steps[0].text, "first");
        assert_eq!(trace_steps[1].text, "second");
    }

    #[test]
    fn describes_codegraph_tool_call_and_result() {
        let tool_call = json!({
            "function": {
                "name": "codegraph_explore",
                "arguments": "{\"query\":\"ChatGroupPage reasoning\",\"maxFiles\":4}"
            }
        });
        let tool_message = json!({
            "role": "tool",
            "content": "CodeGraph explore note:\nFound 8 symbols across 2 files."
        });

        let call_step = tool_call_trace_step(&tool_call);
        let result_step = tool_result_trace_step(&tool_call, &tool_message);

        assert_eq!(call_step.kind, "tool");
        assert!(call_step.text.contains("ChatGroupPage reasoning"));
        assert!(call_step.text.contains("maxFiles=4"));
        assert!(result_step.text.contains("via CodeGraph"));
    }

    #[test]
    fn finds_sse_event_separators() {
        assert_eq!(sse_event_separator("data: one\n\nrest"), Some((9, 2)));
        assert_eq!(sse_event_separator("data: one\r\n\r\nrest"), Some((9, 4)));
        assert_eq!(sse_event_separator("data: one"), None);
    }

    #[test]
    fn accumulates_streamed_tool_call_arguments() {
        let mut tool_calls = Vec::new();

        append_delta_tool_calls(
            &[json!({
                "index": 0,
                "id": "call_1",
                "type": "function",
                "function": {
                    "name": "codegraph_",
                    "arguments": "{\"query\":\"Chat"
                }
            })],
            &mut tool_calls,
        );
        append_delta_tool_calls(
            &[json!({
                "index": 0,
                "function": {
                    "name": "explore",
                    "arguments": "GroupPage\"}"
                }
            })],
            &mut tool_calls,
        );

        let values = tool_call_accumulators_to_values(tool_calls);

        assert_eq!(values.len(), 1);
        assert_eq!(values[0]["id"], json!("call_1"));
        assert_eq!(values[0]["function"]["name"], json!("codegraph_explore"));
        assert_eq!(
            values[0]["function"]["arguments"],
            json!("{\"query\":\"ChatGroupPage\"}")
        );
    }

    #[test]
    fn reads_first_choice_finish_reason() {
        let parsed = json!({
            "choices": [
                {
                    "finish_reason": "tool_calls",
                    "message": { "content": "" }
                }
            ]
        });

        assert_eq!(
            first_choice_finish_reason(&parsed),
            Some("tool_calls".to_string())
        );
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            load_app_cache,
            save_app_cache,
            copy_avatar_to_cache,
            chat_completion,
            inspect_code_workspace,
            apply_patch_proposal
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
