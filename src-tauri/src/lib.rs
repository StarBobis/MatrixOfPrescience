use base64::{engine::general_purpose, Engine as _};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};
use tauri::{AppHandle, Emitter, Manager};

mod tools;

use tools::{
    code_tools_schema, execute_code_tool_call, tool_call_trace_step, tool_result_trace_step,
    validate_workspace,
};

#[cfg(test)]
use tools::{
    delete_workspace_path_tool, format_codegraph_explore_output, is_codegraph_status_query,
    move_workspace_path_tool, normalize_codegraph_max_files, read_workspace_file_tool,
    resolve_workspace_relative_path, strip_ansi_escape_sequences, write_workspace_file_tool,
    DEFAULT_CODEGRAPH_MAX_FILES, MAX_CODEGRAPH_MAX_FILES,
};

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
    can_write: Option<bool>,
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
pub(crate) struct ChatTraceStep {
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

const MAX_CODE_TOOL_ROUNDS: usize = 8;
const MAX_CHAT_COMPLETION_TURNS: usize = MAX_CODE_TOOL_ROUNDS + 2;
const MAX_CHAT_TRACE_STEPS: usize = 160;
const TRACE_STEP_TEXT_LIMIT: usize = 280;
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
    let can_write = request.can_write.unwrap_or(false);
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
            payload["tools"] = code_tools_schema(is_deepseek, can_write);
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
                payload["tools"] = code_tools_schema(false, can_write);
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
                    let mut stream_tool_output = |step: ChatTraceStep| {
                        emit_tool_chunk(&app, stream_id.as_deref(), &step);
                    };
                    let tool_result = execute_code_tool_call(
                        workspace,
                        tool_call,
                        can_write,
                        Some(&mut stream_tool_output),
                    );
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

pub(crate) fn message_content_text(message: &Value) -> String {
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

fn emit_tool_chunk(app: &AppHandle, stream_id: Option<&str>, step: &ChatTraceStep) {
    emit_stream_event(
        app,
        stream_id,
        "toolChunk",
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
    fn chat_turn_budget_leaves_room_for_final_answer_after_tools() {
        assert!(MAX_CHAT_COMPLETION_TURNS > MAX_CODE_TOOL_ROUNDS);
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
        let schema = code_tools_schema(true, true);
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
    fn code_tools_schema_hides_write_tools_without_permission() {
        let schema = code_tools_schema(true, false);
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
        assert!(!names.contains(&"write_file"));
        assert!(!names.contains(&"create_directory"));
        assert!(!names.contains(&"delete_path"));
        assert!(!names.contains(&"move_path"));
        assert!(!names.contains(&"apply_patch"));
        assert!(!names.contains(&"run_command"));
    }

    #[test]
    fn execute_code_tool_call_blocks_write_tools_without_permission() {
        let tool_result = execute_code_tool_call(
            Path::new("."),
            &json!({
                "id": "call-run",
                "function": {
                    "name": "run_command",
                    "arguments": "{\"command\":\"echo\",\"args\":[\"should-not-run\"]}"
                }
            }),
            false,
            None,
        );

        let content = tool_result
            .get("content")
            .and_then(Value::as_str)
            .unwrap_or_default();

        assert!(content.contains("write permission is disabled"));
        assert!(!content.contains("should-not-run"));
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
            tools::inspect_code_workspace,
            tools::apply_patch_proposal
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
