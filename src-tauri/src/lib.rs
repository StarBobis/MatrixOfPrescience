use base64::{engine::general_purpose, Engine as _};
use futures_util::StreamExt;
use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tauri::{AppHandle, Emitter, Manager, State};

mod dsml;
mod tools;
mod utils;
mod validation;

use dsml::normalize_dsml_tool_calls_in_message;
use tools::{
    code_tools_schema, execute_code_tool_call, orchestration_tools_schema, tool_call_trace_step,
    tool_result_trace_step, validate_workspace,
};
use utils::agent_turn_utils::{AgentReflectionDecision, AgentTurnPhase, AgentTurnState};
use utils::tool_call_utils::ToolCallUtils;
use validation::{
    ValidationOps, ValidationRun, ValidationState, VALIDATION_FAILURE_RECOVERY_INSTRUCTION,
    VALIDATION_REQUIRED_INSTRUCTION, VALIDATION_UNAVAILABLE_INSTRUCTION,
};

// Re-export shared struct types for use by other modules
pub(crate) use utils::fs_utils::FileUtils;
pub(crate) use utils::string_utils::StrUtils;
pub(crate) use utils::trace_utils::{ChatCompletionUsage, ChatTraceStep, TraceCtx};

#[cfg(test)]
use tools::{
    delete_workspace_path_tool, format_codegraph_explore_output, is_codegraph_status_query,
    move_workspace_path_tool, normalize_codegraph_max_files, read_workspace_file_tool,
    resolve_workspace_relative_path, write_workspace_file_tool, DEFAULT_CODEGRAPH_MAX_FILES,
    MAX_CODEGRAPH_MAX_FILES,
};

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct ChatMessage {
    role: String,
    content: String,
    #[serde(default, alias = "reasoning_content")]
    reasoning_content: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ChatCompletionRequest {
    provider_name: String,
    base_url: String,
    api_key: String,
    model: String,
    wire_api: Option<String>,
    reasoning_effort: Option<String>,
    temperature: Option<f32>,
    system_prompt: Option<String>,
    workspace_path: Option<String>,
    code_tools_enabled: Option<bool>,
    orchestration_tools_enabled: Option<bool>,
    orchestration_required: Option<bool>,
    can_write: Option<bool>,
    stream_id: Option<String>,
    cancellation_id: Option<String>,
    messages: Vec<ChatMessage>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ChatCompletionResponse {
    content: String,
    trace_steps: Vec<ChatTraceStep>,
    usage: Option<ChatCompletionUsage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    dispatched_tasks: Option<Vec<TaskDispatchedEntry>>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct TaskDispatchedEntry {
    member: String,
    instruction: String,
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
    retry_attempt: Option<usize>,
    retry_delay_ms: Option<u64>,
    retry_reason: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum HttpRetryProgress {
    Waiting {
        attempt: usize,
        delay: Duration,
        reason: String,
    },
    Recovered {
        attempts: usize,
    },
}

#[derive(Debug, Default)]
struct ToolCallAccumulator {
    id: String,
    call_type: String,
    function_name: String,
    function_arguments: String,
}

#[derive(Default)]
struct ChatCancellationState {
    tokens: Mutex<HashMap<String, Arc<AtomicBool>>>,
}

impl ChatCancellationState {
    fn token(&self, cancellation_id: &str) -> Arc<AtomicBool> {
        let mut tokens = self
            .tokens
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        tokens
            .entry(cancellation_id.to_string())
            .or_insert_with(|| Arc::new(AtomicBool::new(false)))
            .clone()
    }

    fn cancel(&self, cancellation_id: &str) {
        let token = self.token(cancellation_id);
        token.store(true, Ordering::Release);
    }

    fn finish(&self, cancellation_id: &str) {
        self.tokens
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(cancellation_id);
    }
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

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct CcSwitchOpenAiConfig {
    source: String,
    provider_name: Option<String>,
    base_url: String,
    api_key: String,
    model: Option<String>,
    wire_api: Option<String>,
    warning: Option<String>,
}

const MAX_CHAT_COMPLETION_TURNS: usize = 32;
#[cfg(test)]
const MAX_EDIT_RECOVERY_TOOL_ROUNDS: usize = 4;
const CHAT_COMPLETION_STREAM_EVENT: &str = "chat-completion-stream";
const HTTP_RETRY_DELAY: Duration = Duration::from_secs(5);
const RETRY_CANCELLATION_POLL_INTERVAL: Duration = Duration::from_millis(250);
const FINAL_ANSWER_INSTRUCTION: &str =
    "Use the tool results already provided and write the final answer now.";
const CONTINUE_OUTPUT_INSTRUCTION: &str =
    "Continue exactly from where you left off. Do not restart, repeat, or summarize prior text. Finish the same answer.";
const EDIT_FAILURE_RECOVERY_INSTRUCTION: &str = "The previous edit tool call failed. Do not stop or provide a final answer solely because an edit did not apply. Recover using the error and the current workspace state: re-read the target when the context may be stale, then retry with a corrected smaller patch or a different available edit tool. Do not repeat the identical failing call. Continue until the requested change is complete or no available tool can resolve a genuine blocker.";
const MAX_TOOL_ONLY_ROUNDS: usize = 6;
const MAX_TOTAL_TOOL_CALLS_PER_AUTONOMOUS_ROUND: usize = 24;
const MAX_DEEPSEEK_TOOL_CALLS_PER_TURN: usize = 1;
const TOOL_CALL_CHECKPOINT_INTERVAL: usize = 8;
const TOOL_CALL_CHECKPOINT_REASONING_EFFORT: &str = "high";
const TOOL_CALL_CHECKPOINT_INSTRUCTION: &str = "Checkpoint: the tool-call budget for this step has been reached. On this turn, do not call any tools. Briefly summarize what you have learned, what remains uncertain, and the single best next step. Keep it concise. After this checkpoint, continue the task on the following turn without waiting for user input if more work is needed.";
const DEEPSEEK_TOOL_ACTION_INSTRUCTION: &str = "This is a tool action turn. If more work is needed, call the single most useful tool now, guided by the latest reflection and tool results. If the task is already complete or no tool is needed, do not call a tool — write the final answer now, starting it with `FINAL:` when you can.";
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

fn user_home_dir() -> Option<PathBuf> {
    std::env::var("USERPROFILE")
        .ok()
        .and_then(|value| StrUtils::trim_non_empty(&value))
        .map(PathBuf::from)
        .or_else(|| {
            let drive = std::env::var("HOMEDRIVE").ok()?;
            let path = std::env::var("HOMEPATH").ok()?;
            StrUtils::trim_non_empty(&format!("{}{}", drive, path)).map(PathBuf::from)
        })
        .or_else(|| {
            std::env::var("HOME")
                .ok()
                .and_then(|value| StrUtils::trim_non_empty(&value))
                .map(PathBuf::from)
        })
}

fn codex_config_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();

    if let Ok(codex_home) = std::env::var("CODEX_HOME") {
        if let Some(path) = StrUtils::trim_non_empty(&codex_home).map(PathBuf::from) {
            dirs.push(path);
        }
    }

    if let Some(home) = user_home_dir() {
        let default_dir = home.join(".codex");
        if !dirs.iter().any(|path| path == &default_dir) {
            dirs.push(default_dir);
        }
    }

    dirs
}

fn ccswitch_config_dir() -> Result<PathBuf, String> {
    let Some(home) = user_home_dir() else {
        return Err("Unable to resolve user home directory.".to_string());
    };

    let default_dir = home.join(".cc-switch");

    #[cfg(windows)]
    {
        let default_db = default_dir.join("cc-switch.db");
        if !default_db.exists() {
            if let Ok(home_env) = std::env::var("HOME") {
                if let Some(legacy_home) = StrUtils::trim_non_empty(&home_env).map(PathBuf::from) {
                    let legacy_dir = legacy_home.join(".cc-switch");
                    if legacy_dir.join("cc-switch.db").exists() {
                        return Ok(legacy_dir);
                    }
                }
            }
        }
    }

    Ok(default_dir)
}

fn active_codex_provider<'a>(doc: &'a toml::Value) -> (Option<String>, Option<&'a toml::Value>) {
    let active_provider = doc
        .get("model_provider")
        .and_then(toml::Value::as_str)
        .and_then(StrUtils::trim_non_empty);
    let active_provider_config = active_provider.as_deref().and_then(|provider| {
        doc.get("model_providers")
            .and_then(|providers| providers.get(provider))
    });

    (active_provider, active_provider_config)
}

fn codex_toml_field(doc: Option<&toml::Value>, key: &str) -> Option<String> {
    let Some(doc) = doc else {
        return None;
    };
    let (_, active_provider_config) = active_codex_provider(doc);

    StrUtils::toml_str(active_provider_config, key).or_else(|| StrUtils::toml_str(Some(doc), key))
}

fn codex_auth_api_key(auth: Option<&Value>) -> Option<String> {
    StrUtils::json_str(auth, "OPENAI_API_KEY")
}

fn is_local_ccswitch_proxy_url(base_url: &str) -> bool {
    let lower = base_url.trim().trim_end_matches('/').to_ascii_lowercase();
    let is_local = lower.starts_with("http://127.0.0.1:")
        || lower.starts_with("http://localhost:")
        || lower.starts_with("http://[::1]:");

    is_local
        && (lower.contains("/codex")
            || lower.starts_with("http://127.0.0.1:15721")
            || lower.starts_with("http://localhost:15721"))
}

fn normalize_imported_base_url(base_url: &str) -> String {
    let trimmed = base_url.trim().trim_end_matches('/').to_string();
    let lower = trimmed.to_ascii_lowercase();

    if is_local_ccswitch_proxy_url(&trimmed) && lower.ends_with("/codex") {
        return format!("{}/v1", trimmed);
    }

    trimmed
}

fn imported_config_warning(
    base_url: &str,
    wire_api: Option<&str>,
    provider_type: Option<&str>,
) -> Option<String> {
    if is_local_ccswitch_proxy_url(base_url) {
        return None;
    }

    if provider_type == Some("codex_oauth") {
        return Some("codexOAuth".to_string());
    }

    let wire_api = wire_api.map(|value| value.to_ascii_lowercase());
    if matches!(wire_api.as_deref(), Some("responses" | "openai_responses")) {
        return Some("responses".to_string());
    }

    None
}

fn build_ccswitch_openai_config(
    source: String,
    provider_name: Option<String>,
    config_text: &str,
    auth: Option<&Value>,
    settings: Option<&Value>,
    meta: Option<&Value>,
) -> Result<CcSwitchOpenAiConfig, String> {
    let doc = config_text.parse::<toml::Value>().ok();
    let active_provider_name = doc.as_ref().and_then(|doc| {
        let (active_provider, active_provider_config) = active_codex_provider(doc);
        StrUtils::toml_str(active_provider_config, "name").or(active_provider)
    });
    let provider_name = provider_name.or(active_provider_name);
    let base_url = codex_toml_field(doc.as_ref(), "base_url")
        .or_else(|| StrUtils::json_str(settings, "baseUrl"))
        .or_else(|| StrUtils::json_str(settings, "base_url"))
        .ok_or_else(|| "Codex config does not contain a base_url.".to_string())?;
    let base_url = normalize_imported_base_url(&base_url);
    let wire_api = codex_toml_field(doc.as_ref(), "wire_api")
        .or_else(|| StrUtils::json_str(meta, "apiFormat"))
        .or_else(|| StrUtils::json_str(meta, "api_format"));
    let provider_type = StrUtils::json_str(meta, "providerType")
        .or_else(|| StrUtils::json_str(meta, "provider_type"));
    let mut api_key = codex_toml_field(doc.as_ref(), "experimental_bearer_token")
        .or_else(|| codex_auth_api_key(auth))
        .or_else(|| StrUtils::json_str(settings, "apiKey"))
        .or_else(|| StrUtils::json_str(settings, "api_key"));

    if api_key.is_none() && is_local_ccswitch_proxy_url(&base_url) {
        api_key = Some("cc-switch-local-proxy".to_string());
    }

    let api_key =
        api_key.ok_or_else(|| "Codex config does not contain an OpenAI API key.".to_string())?;
    let model =
        codex_toml_field(doc.as_ref(), "model").or_else(|| StrUtils::json_str(settings, "model"));
    let warning = imported_config_warning(&base_url, wire_api.as_deref(), provider_type.as_deref());

    Ok(CcSwitchOpenAiConfig {
        source,
        provider_name,
        base_url,
        api_key,
        model,
        wire_api,
        warning,
    })
}

fn load_ccswitch_openai_config_from_codex_live() -> Result<CcSwitchOpenAiConfig, String> {
    let mut failures = Vec::new();

    for config_dir in codex_config_dirs() {
        let config_path = config_dir.join("config.toml");
        let auth_path = config_dir.join("auth.json");

        if !config_path.exists() && !auth_path.exists() {
            continue;
        }

        let config_text = if config_path.exists() {
            fs::read_to_string(&config_path)
                .map_err(|error| format!("Failed to read {:?}: {}", config_path, error))?
        } else {
            String::new()
        };
        let auth = read_json_file(&auth_path).ok().flatten();

        match build_ccswitch_openai_config(
            "Codex live config".to_string(),
            None,
            &config_text,
            auth.as_ref(),
            None,
            None,
        ) {
            Ok(config) => return Ok(config),
            Err(error) => failures.push(format!("{}: {}", config_path.display(), error)),
        }
    }

    if failures.is_empty() {
        Err("No Codex live config was found under CODEX_HOME or ~/.codex.".to_string())
    } else {
        Err(failures.join("; "))
    }
}

fn load_ccswitch_openai_config_from_database() -> Result<CcSwitchOpenAiConfig, String> {
    let db_path = ccswitch_config_dir()?.join("cc-switch.db");
    if !db_path.exists() {
        return Err(format!(
            "CC Switch database was not found at {:?}.",
            db_path
        ));
    }

    let conn = Connection::open_with_flags(
        &db_path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|error| format!("Failed to open CC Switch database {:?}: {}", db_path, error))?;

    let (provider_name, settings_config, meta): (String, String, String) = conn
        .query_row(
            "SELECT name, settings_config, meta FROM providers WHERE app_type = 'codex' AND is_current = 1 LIMIT 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .map_err(|error| format!("Failed to read current Codex provider from CC Switch database: {}", error))?;

    let settings: Value = serde_json::from_str(&settings_config)
        .map_err(|error| format!("Failed to parse CC Switch provider settings: {}", error))?;
    let meta: Value = serde_json::from_str(&meta).unwrap_or_else(|_| json!({}));
    let config_text = settings
        .get("config")
        .and_then(Value::as_str)
        .unwrap_or_default();

    build_ccswitch_openai_config(
        "CC Switch database".to_string(),
        Some(provider_name),
        config_text,
        settings.get("auth"),
        Some(&settings),
        Some(&meta),
    )
}

#[tauri::command]
fn load_ccswitch_openai_config() -> Result<CcSwitchOpenAiConfig, String> {
    let mut failures = Vec::new();

    match load_ccswitch_openai_config_from_codex_live() {
        Ok(config) => return Ok(config),
        Err(error) => failures.push(error),
    }

    match load_ccswitch_openai_config_from_database() {
        Ok(config) => return Ok(config),
        Err(error) => failures.push(error),
    }

    Err(format!(
        "No compatible OpenAI Chat configuration was found in local CC Switch/Codex config. {}",
        failures.join("; ")
    ))
}

fn chat_completions_endpoint(base_url: &str) -> String {
    let normalized = normalize_imported_base_url(base_url);
    let lower = normalized.to_ascii_lowercase();

    if lower.ends_with("/chat/completions") {
        return normalized;
    }

    format!("{}/chat/completions", normalized.trim_end_matches('/'))
}

fn responses_endpoint(base_url: &str) -> String {
    let normalized = normalize_imported_base_url(base_url);
    let trimmed = normalized.trim_end_matches('/');
    let lower = trimmed.to_ascii_lowercase();

    if lower.ends_with("/responses") {
        return trimmed.to_string();
    }

    if lower.ends_with("/chat/completions") {
        let base = &trimmed[..trimmed.len() - "/chat/completions".len()];
        return format!("{}/responses", base.trim_end_matches('/'));
    }

    format!("{}/responses", trimmed)
}

fn is_openai_reasoning_model(model: &str) -> bool {
    let model = model.trim().to_ascii_lowercase();
    model.starts_with("gpt-5")
        || model.starts_with("o1")
        || model.starts_with("o3")
        || model.starts_with("o4")
}

fn should_use_responses_api(request: &ChatCompletionRequest, is_deepseek: bool) -> bool {
    if is_deepseek
        || !reasoning_enabled(request.reasoning_effort.as_deref())
        || !is_openai_reasoning_model(&request.model)
    {
        return false;
    }

    let wire_api = request
        .wire_api
        .as_deref()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();

    if wire_api.contains("chat") {
        return false;
    }

    wire_api.contains("responses")
        || is_local_ccswitch_proxy_url(&request.base_url)
        || request
            .base_url
            .to_ascii_lowercase()
            .contains("api.openai.com")
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
    cancellation_state: State<'_, ChatCancellationState>,
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
    let cancellation = request
        .cancellation_id
        .as_deref()
        .map(|cancellation_id| cancellation_state.token(cancellation_id));

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
        let mut msg = json!({
            "role": message.role,
            "content": message.content,
        });
        if let Some(reasoning) = &message.reasoning_content {
            if !reasoning.trim().is_empty() {
                msg["reasoning_content"] = json!(reasoning);
            }
        }
        messages.push(msg);
    }

    let is_deepseek = is_deepseek_provider(&request.provider_name, &request.base_url);

    if should_use_responses_api(&request, is_deepseek) {
        return openai_responses_completion(app, request, code_workspace, cancellation).await;
    }

    let endpoint = chat_completions_endpoint(&request.base_url);
    let client = reqwest::Client::new();
    let deepseek_reasoning_requested =
        is_deepseek && reasoning_enabled(request.reasoning_effort.as_deref());
    let deepseek_tool_workflow = is_deepseek
        && (request.code_tools_enabled.unwrap_or(false)
            || request.orchestration_tools_enabled.unwrap_or(false));
    let mut agent_turn_state = AgentTurnState::new(
        is_deepseek,
        deepseek_reasoning_requested,
        deepseek_tool_workflow,
    );
    let max_chat_completion_turns = agent_turn_state.max_turns(MAX_CHAT_COMPLETION_TURNS);
    let can_write = request.can_write.unwrap_or(false);
    let max_tool_calls_per_turn = if is_deepseek {
        MAX_DEEPSEEK_TOOL_CALLS_PER_TURN
    } else {
        TOOL_CALL_CHECKPOINT_INTERVAL
    };
    let mut code_tool_called = false;
    let mut edit_recovery_required = false;
    let mut edit_recovery_rounds = 0usize;
    let mut final_answer_requested = false;
    let mut tool_only_rounds: usize = 0;
    let mut tool_calls_since_checkpoint = 0usize;
    let mut total_tool_calls_executed = 0usize;
    let mut dispatched_tasks: Option<Vec<TaskDispatchedEntry>> = None;
    let mut tool_checkpoint_pending = false;
    let mut tool_budget_reset_pending = false;
    let mut validation = ValidationState::default();
    let mut last_finish_reason: Option<String> = None;
    let mut last_usage: Option<ChatCompletionUsage> = None;
    let mut trace_steps: Vec<ChatTraceStep> = Vec::new();
    let mut accumulated_display_content = String::new();
    let stream_id = request
        .stream_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);

    for turn_index in 0..max_chat_completion_turns {
        let validation_tool_required = validation.requires_tool(edit_recovery_required);
        if validation_tool_required {
            validation.mark_model_prompted();
        }
        let repair_required = edit_recovery_required || validation.requires_repair();
        let turn_phase = agent_turn_state.phase(
            final_answer_requested,
            tool_checkpoint_pending,
            validation_tool_required,
            repair_required,
        );
        let checkpoint_required = matches!(
            turn_phase,
            AgentTurnPhase::ToolReflection | AgentTurnPhase::BudgetCheckpoint
        );
        let phase_instruction = match turn_phase {
            AgentTurnPhase::ToolReflection => Some(AgentTurnState::DEEPSEEK_REFLECTION_INSTRUCTION),
            AgentTurnPhase::BudgetCheckpoint => Some(TOOL_CALL_CHECKPOINT_INSTRUCTION),
            AgentTurnPhase::ToolAction if deepseek_tool_workflow => {
                Some(DEEPSEEK_TOOL_ACTION_INSTRUCTION)
            }
            _ => None,
        };
        let payload_messages = chat_payload_messages(
            &messages,
            final_answer_requested && !validation_tool_required && !repair_required,
            validation_tool_required,
            phase_instruction,
        );
        let suppress_content_stream = is_deepseek
            && deepseek_tool_workflow
            && !matches!(
                turn_phase,
                AgentTurnPhase::Conversation | AgentTurnPhase::FinalAnswer
            );
        let mut payload = json!({
            "model": request.model,
            "messages": payload_messages,
            "temperature": request.temperature.unwrap_or(0.7),
        });
        let tools_blocked = turn_phase.blocks_tools();
        let orchestration_required = request.orchestration_required.unwrap_or(false);
        let code_tools_allowed = !tools_blocked
            && code_workspace.is_some()
            && !orchestration_required;
        let orchestration_tools_allowed =
            !tools_blocked && request.orchestration_tools_enabled.unwrap_or(false);
        let any_tools_allowed = code_tools_allowed || orchestration_tools_allowed;
        let strict_tool_schema = is_deepseek && any_tools_allowed;

        let reasoning_effort = agent_turn_state.reasoning_effort(
            turn_phase,
            request.reasoning_effort.as_deref(),
            TOOL_CALL_CHECKPOINT_REASONING_EFFORT,
        );
        apply_reasoning_payload(&mut payload, is_deepseek, reasoning_effort);
        if turn_phase == AgentTurnPhase::ToolReflection {
            payload["max_tokens"] = json!(AgentTurnState::DEEPSEEK_REFLECTION_MAX_TOKENS);
        }

        if any_tools_allowed {
            let mut tools: Vec<Value> = Vec::new();
            if code_tools_allowed {
                if let Value::Array(code_tools) = code_tools_schema(is_deepseek, can_write) {
                    tools.extend(code_tools);
                }
            }
            if orchestration_tools_allowed {
                if let Value::Array(orchestration_tools) = orchestration_tools_schema(is_deepseek) {
                    tools.extend(orchestration_tools);
                }
            }
            payload["tools"] = Value::Array(tools);
            if let Some(tool_choice) = agent_turn_state.tool_choice(
                turn_phase,
                validation_tool_required,
                repair_required,
                orchestration_required,
            ) {
                payload["tool_choice"] = tool_choice;
            }
        }

        let parsed = match send_chat_completion_request_maybe_stream(
            &app,
            stream_id.as_deref(),
            &client,
            &endpoint,
            &request.api_key,
            &request.provider_name,
            &payload,
            cancellation.as_deref(),
            suppress_content_stream,
        )
        .await
        {
            Ok(parsed) => parsed,
            Err(error) if strict_tool_schema => {
                let mut fallback_tools: Vec<Value> = Vec::new();
                if code_tools_allowed {
                    if let Value::Array(code_tools) = code_tools_schema(false, can_write) {
                        fallback_tools.extend(code_tools);
                    }
                }
                if orchestration_tools_allowed {
                    if let Value::Array(orchestration_tools) = orchestration_tools_schema(false) {
                        fallback_tools.extend(orchestration_tools);
                    }
                }
                payload["tools"] = Value::Array(fallback_tools);
                if let Some(tool_choice) = agent_turn_state.tool_choice(
                    turn_phase,
                    validation_tool_required,
                    repair_required,
                    orchestration_required,
                ) {
                    payload["tool_choice"] = tool_choice;
                } else {
                    if let Some(object) = payload.as_object_mut() {
                        object.remove("tool_choice");
                    }
                }
                send_chat_completion_request_maybe_stream(
                    &app,
                    stream_id.as_deref(),
                    &client,
                    &endpoint,
                    &request.api_key,
                    &request.provider_name,
                    &payload,
                    cancellation.as_deref(),
                    suppress_content_stream,
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
        if let Some(usage) = TraceCtx::usage_from(&parsed) {
            last_usage = Some(usage);
        }
        last_finish_reason = first_choice_finish_reason(&parsed);
        let message = parsed
            .get("choices")
            .and_then(|choices| choices.get(0))
            .and_then(|choice| choice.get("message"))
            .cloned()
            .ok_or_else(|| format!("{} returned no message.", request.provider_name))?;
        let message = normalize_dsml_tool_calls_in_message(message);
        TraceCtx::append_reasoning(&mut trace_steps, &message);

        if !checkpoint_required {
            if let (Some(workspace), Some(tool_calls)) = (
                code_workspace.as_ref(),
                message.get("tool_calls").and_then(Value::as_array),
            ) {
                if !tool_calls.is_empty() {
                    let remaining_total_tool_budget = MAX_TOTAL_TOOL_CALLS_PER_AUTONOMOUS_ROUND
                        .saturating_sub(total_tool_calls_executed);
                    // Hard-cap a single assistant tool batch so one response cannot skip the checkpoint.
                    let tool_execution_plan = ToolCallUtils::plan_execution(
                        tool_calls.len(),
                        tool_calls_since_checkpoint,
                        TOOL_CALL_CHECKPOINT_INTERVAL,
                        max_tool_calls_per_turn,
                        remaining_total_tool_budget,
                    );
                    if tool_execution_plan.executable_count == 0 {
                        if ToolCallUtils::can_open_next_autonomous_round(
                            turn_index,
                            max_chat_completion_turns,
                        ) {
                            tool_checkpoint_pending = true;
                            tool_budget_reset_pending = true;
                            final_answer_requested = false;
                        } else {
                            tool_checkpoint_pending = false;
                            final_answer_requested = true;
                        }
                        continue;
                    }
                    let executable_tool_calls = &tool_calls[..tool_execution_plan.executable_count];
                    messages.push(ToolCallUtils::trim_chat_tool_calls(
                        &message,
                        tool_execution_plan.executable_count,
                    ));
                    let mut failed_edit = false;
                    let mut successful_edit = false;
                    let mut validation_run = ValidationRun::default();
                    for tool_call in executable_tool_calls {
                        let call_step = tool_call_trace_step(tool_call);
                        emit_trace_step(&app, stream_id.as_deref(), &call_step);
                        TraceCtx::append_steps(&mut trace_steps, vec![call_step]);
                        let tool_result = if let Some(tool_result) =
                            validation.redundant_validation_tool_result(tool_call)
                        {
                            tool_result
                        } else {
                            let mut stream_tool_output = |step: ChatTraceStep| {
                                emit_tool_chunk(&app, stream_id.as_deref(), &step);
                            };
                            execute_code_tool_call(
                                workspace,
                                tool_call,
                                can_write,
                                Some(&mut stream_tool_output),
                            )
                        };
                        validation_run.observe_tool_result(tool_call, &tool_result);
                        if dispatched_tasks.is_none() {
                            if let Some(entries) = extract_dispatched_tasks(tool_call, &tool_result) {
                                dispatched_tasks = Some(entries);
                            }
                        }
                        if ValidationOps::result_succeeded(&tool_result)
                            && ValidationOps::is_edit_call(tool_call)
                        {
                            successful_edit = true;
                        }
                        if ValidationOps::edit_needs_recovery(tool_call, &tool_result) {
                            failed_edit = true;
                        }
                        let result_step = tool_result_trace_step(tool_call, &tool_result);
                        emit_trace_step(&app, stream_id.as_deref(), &result_step);
                        TraceCtx::append_steps(&mut trace_steps, vec![result_step]);
                        messages.push(tool_result);
                    }
                    (edit_recovery_required, edit_recovery_rounds) =
                        ValidationOps::next_recovery_state(
                            edit_recovery_required,
                            edit_recovery_rounds,
                            failed_edit,
                            successful_edit,
                        );
                    if failed_edit && edit_recovery_required {
                        messages.push(json!({
                            "role": "user",
                            "content": EDIT_FAILURE_RECOVERY_INSTRUCTION,
                        }));
                    }
                    let validation_failed = if successful_edit {
                        validation.mark_successful_edit();
                        false
                    } else {
                        validation.record_run(validation_run)
                    };
                    if validation_failed {
                        messages.push(json!({
                            "role": "user",
                            "content": VALIDATION_FAILURE_RECOVERY_INSTRUCTION,
                        }));
                    }
                    if should_finish_after_dispatch_tasks(
                        dispatched_tasks.as_deref(),
                        &validation,
                        edit_recovery_required,
                    ) {
                        let content = dispatched_tasks
                            .as_deref()
                            .map(dispatched_tasks_completion_content)
                            .unwrap_or_default();
                        return Ok(ChatCompletionResponse {
                            content,
                            trace_steps,
                            usage: last_usage,
                            dispatched_tasks: dispatched_tasks.take(),
                        });
                    }
                    if edit_recovery_required || validation.requires_repair() {
                        final_answer_requested = false;
                    }
                    code_tool_called = true;
                    agent_turn_state.mark_tools_executed();
                    total_tool_calls_executed += tool_execution_plan.executable_count;
                    ToolCallUtils::schedule_checkpoint(
                        &mut tool_calls_since_checkpoint,
                        &mut tool_checkpoint_pending,
                        tool_execution_plan.executable_count,
                        TOOL_CALL_CHECKPOINT_INTERVAL,
                    );
                    if is_deepseek && tool_execution_plan.truncated {
                        tool_checkpoint_pending = true;
                    }
                    if total_tool_calls_executed >= MAX_TOTAL_TOOL_CALLS_PER_AUTONOMOUS_ROUND {
                        if ToolCallUtils::can_open_next_autonomous_round(
                            turn_index,
                            max_chat_completion_turns,
                        ) {
                            tool_checkpoint_pending = true;
                            tool_budget_reset_pending = true;
                            final_answer_requested = false;
                        } else {
                            tool_checkpoint_pending = false;
                            final_answer_requested = true;
                        }
                    }

                    if !validation_tool_required && !repair_required {
                        tool_only_rounds += 1;
                        if !tool_checkpoint_pending && tool_only_rounds >= MAX_TOOL_ONLY_ROUNDS {
                            if ToolCallUtils::can_open_next_autonomous_round(
                                turn_index,
                                max_chat_completion_turns,
                            ) {
                                tool_checkpoint_pending = true;
                                tool_budget_reset_pending = true;
                                final_answer_requested = false;
                            } else {
                                messages.push(json!({
                                    "role": "user",
                                    "content": FINAL_ANSWER_INSTRUCTION,
                                }));
                                final_answer_requested = true;
                            }
                        }
                    }

                    if validation.should_auto_validate(edit_recovery_required) {
                        validation.mark_auto_attempted();
                        let validation_run = code_workspace
                            .as_ref()
                            .map(|workspace| {
                                ValidationOps::run_default(
                                    &app,
                                    stream_id.as_deref(),
                                    workspace,
                                    can_write,
                                    &mut messages,
                                    &mut trace_steps,
                                )
                            })
                            .unwrap_or_default();

                        if !validation_run.ran() {
                            ValidationOps::mark_unavailable(&mut messages);
                            validation.mark_validator_discovery_required();
                            final_answer_requested = false;
                        } else if validation.record_run(validation_run) {
                            messages.push(json!({
                                "role": "user",
                                "content": VALIDATION_FAILURE_RECOVERY_INSTRUCTION,
                            }));
                            final_answer_requested = false;
                        } else {
                            final_answer_requested = false;
                        }
                    }
                    continue;
                }
            }
        }

        let content = StrUtils::message_content_text(&message);
        let reasoning = TraceCtx::message_reasoning(&message);

        let has_visible_output = !content.is_empty() || !reasoning.is_empty();

        if has_visible_output {
            if turn_phase == AgentTurnPhase::ToolReflection {
                messages.push(ToolCallUtils::trim_chat_tool_calls(&message, 0));
                agent_turn_state.complete_reflection();
                tool_checkpoint_pending = false;
                if tool_budget_reset_pending {
                    tool_only_rounds = 0;
                    tool_calls_since_checkpoint = 0;
                    total_tool_calls_executed = 0;
                    tool_budget_reset_pending = false;
                }

                // When content is empty but reasoning_content exists (DeepSeek thinking-only
                // reflection), the effective reflection decision can be extracted from a fallback
                // source since the model's decision is embedded in the thinking tokens.
                let effective_content = if !content.is_empty() {
                    &content
                } else {
                    &reasoning
                };
                match AgentTurnState::parse_reflection(effective_content) {
                    AgentReflectionDecision::Continue => {
                        // After repeated undecided reflections, ask for the final
                        // answer directly instead of looping action/reflection.
                        final_answer_requested = agent_turn_state.record_unresolved_reflection();
                    }
                    AgentReflectionDecision::Finish(content) => {
                        if finish_reason_indicates_truncated_output(last_finish_reason.as_deref()) {
                            append_continued_output(&mut accumulated_display_content, &content);
                            messages.push(json!({
                                "role": "user",
                                "content": CONTINUE_OUTPUT_INSTRUCTION,
                            }));
                            final_answer_requested = true;
                            continue;
                        }
                        return Ok(ChatCompletionResponse {
                            content: combined_output_text(&accumulated_display_content, &content),
                            trace_steps,
                            usage: last_usage,
                            dispatched_tasks: dispatched_tasks.take(),
                        });
                    }
                    AgentReflectionDecision::RequestFinalAnswer => {
                        final_answer_requested = true;
                    }
                }
                continue;
            }
            if turn_phase == AgentTurnPhase::BudgetCheckpoint {
                messages.push(ToolCallUtils::trim_chat_tool_calls(&message, 0));
                tool_checkpoint_pending = false;
                final_answer_requested = false;
                if tool_budget_reset_pending {
                    tool_only_rounds = 0;
                    tool_calls_since_checkpoint = 0;
                    total_tool_calls_executed = 0;
                    tool_budget_reset_pending = false;
                }
                continue;
            }
            if validation.is_pending() {
                if validation.can_auto_validate() {
                    validation.mark_auto_attempted();
                    let validation_run = code_workspace
                        .as_ref()
                        .map(|workspace| {
                            ValidationOps::run_default(
                                &app,
                                stream_id.as_deref(),
                                workspace,
                                can_write,
                                &mut messages,
                                &mut trace_steps,
                            )
                        })
                        .unwrap_or_default();

                    if validation_run.ran() {
                        if validation.record_run(validation_run) {
                            messages.push(json!({
                                "role": "user",
                                "content": VALIDATION_FAILURE_RECOVERY_INSTRUCTION,
                            }));
                            final_answer_requested = false;
                        } else {
                            final_answer_requested = false;
                        }
                        continue;
                    } else {
                        ValidationOps::mark_unavailable(&mut messages);
                        validation.mark_validator_discovery_required();
                        final_answer_requested = false;
                        continue;
                    }
                }
                final_answer_requested = false;
                continue;
            }
            if turn_phase == AgentTurnPhase::ToolAction && deepseek_tool_workflow {
                // The model declined to call a tool on an action turn — let it
                // stop on its own instead of forcing another tool round.
                messages.push(ToolCallUtils::trim_chat_tool_calls(&message, 0));
                if !content.is_empty() {
                    let action_answer = match AgentTurnState::parse_reflection(&content) {
                        AgentReflectionDecision::Finish(answer) => answer,
                        AgentReflectionDecision::RequestFinalAnswer => {
                            final_answer_requested = true;
                            continue;
                        }
                        // Plain prose without a control line is also a stop
                        // decision: accept it as the final answer.
                        AgentReflectionDecision::Continue => content.clone(),
                    };
                    if finish_reason_indicates_truncated_output(last_finish_reason.as_deref()) {
                        append_continued_output(&mut accumulated_display_content, &action_answer);
                        messages.push(json!({
                            "role": "user",
                            "content": CONTINUE_OUTPUT_INSTRUCTION,
                        }));
                        final_answer_requested = true;
                        continue;
                    }
                    return Ok(ChatCompletionResponse {
                        content: combined_output_text(&accumulated_display_content, &action_answer),
                        trace_steps,
                        usage: last_usage,
                        dispatched_tasks: dispatched_tasks.take(),
                    });
                }
                // Thinking-only output with no tool call: ask for the answer
                // directly instead of spinning on more action turns.
                final_answer_requested = true;
                continue;
            }

            if finish_reason_indicates_truncated_output(last_finish_reason.as_deref()) {
                append_continued_output(&mut accumulated_display_content, &content);
                messages.push(message.clone());
                messages.push(json!({
                    "role": "user",
                    "content": CONTINUE_OUTPUT_INSTRUCTION,
                }));
                final_answer_requested = true;
                continue;
            }

            return Ok(ChatCompletionResponse {
                content: combined_output_text(&accumulated_display_content, &content),
                trace_steps,
                usage: last_usage,
                dispatched_tasks: dispatched_tasks.take(),
            });
        }

        if turn_phase == AgentTurnPhase::ToolReflection {
            // Store the message even when content is empty while reasoning_content is present,
            // so DeepSeek's thinking-mode context is preserved across turns.
            if !reasoning.is_empty() {
                messages.push(ToolCallUtils::trim_chat_tool_calls(&message, 0));
                match AgentTurnState::parse_reflection(&reasoning) {
                    AgentReflectionDecision::Continue => {
                        // After repeated undecided reflections, ask for the final
                        // answer directly instead of looping action/reflection.
                        final_answer_requested = agent_turn_state.record_unresolved_reflection();
                    }
                    AgentReflectionDecision::Finish(finish_content) => {
                        if finish_reason_indicates_truncated_output(last_finish_reason.as_deref()) {
                            append_continued_output(
                                &mut accumulated_display_content,
                                &finish_content,
                            );
                            messages.push(json!({
                                "role": "user",
                                "content": CONTINUE_OUTPUT_INSTRUCTION,
                            }));
                            final_answer_requested = true;
                        } else {
                            return Ok(ChatCompletionResponse {
                                content: combined_output_text(
                                    &accumulated_display_content,
                                    &finish_content,
                                ),
                                trace_steps,
                                usage: last_usage,
                                dispatched_tasks: dispatched_tasks.take(),
                            });
                        }
                    }
                    AgentReflectionDecision::RequestFinalAnswer => {
                        final_answer_requested = true;
                    }
                }
                agent_turn_state.complete_reflection();
                tool_checkpoint_pending = false;
                if tool_budget_reset_pending {
                    tool_only_rounds = 0;
                    tool_calls_since_checkpoint = 0;
                    total_tool_calls_executed = 0;
                    tool_budget_reset_pending = false;
                }
                continue;
            }
            agent_turn_state.complete_reflection();
            tool_checkpoint_pending = false;
            final_answer_requested = false;
            if tool_budget_reset_pending {
                tool_only_rounds = 0;
                tool_calls_since_checkpoint = 0;
                total_tool_calls_executed = 0;
                tool_budget_reset_pending = false;
            }
            continue;
        }

        if turn_phase == AgentTurnPhase::BudgetCheckpoint {
            tool_checkpoint_pending = false;
            final_answer_requested = false;
            if tool_budget_reset_pending {
                tool_only_rounds = 0;
                tool_calls_since_checkpoint = 0;
                total_tool_calls_executed = 0;
                tool_budget_reset_pending = false;
            }
            continue;
        }

        if turn_phase == AgentTurnPhase::ToolAction && deepseek_tool_workflow {
            // Empty action-turn output: ask for the answer directly instead of
            // spinning on more action turns.
            final_answer_requested = true;
            continue;
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

#[tauri::command(rename_all = "camelCase")]
fn cancel_chat_completion(
    cancellation_id: String,
    cancellation_state: State<'_, ChatCancellationState>,
) {
    cancellation_state.cancel(cancellation_id.trim());
}

#[tauri::command(rename_all = "camelCase")]
fn finish_chat_completion(
    cancellation_id: String,
    cancellation_state: State<'_, ChatCancellationState>,
) {
    cancellation_state.finish(cancellation_id.trim());
}

async fn openai_responses_completion(
    app: AppHandle,
    request: ChatCompletionRequest,
    code_workspace: Option<PathBuf>,
    cancellation: Option<Arc<AtomicBool>>,
) -> Result<ChatCompletionResponse, String> {
    let endpoint = responses_endpoint(&request.base_url);
    let client = reqwest::Client::new();
    let can_write = request.can_write.unwrap_or(false);
    let is_deepseek = is_deepseek_provider(&request.provider_name, &request.base_url);
    let max_tool_calls_per_turn = if is_deepseek {
        MAX_DEEPSEEK_TOOL_CALLS_PER_TURN
    } else {
        TOOL_CALL_CHECKPOINT_INTERVAL
    };
    let mut code_tool_called = false;
    let mut edit_recovery_required = false;
    let mut edit_recovery_rounds = 0usize;
    let mut final_answer_requested = false;
    let mut tool_only_rounds: usize = 0;
    let mut tool_calls_since_checkpoint = 0usize;
    let mut total_tool_calls_executed = 0usize;
    let mut tool_checkpoint_pending = false;
    let mut tool_budget_reset_pending = false;
    let mut validation = ValidationState::default();
    let mut previous_response_id: Option<String> = None;
    let mut pending_input: Vec<Value> = Vec::new();
    let mut last_usage: Option<ChatCompletionUsage> = None;
    let mut trace_steps: Vec<ChatTraceStep> = Vec::new();
    let mut accumulated_display_content = String::new();
    let stream_id = request
        .stream_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);

    for turn_index in 0..MAX_CHAT_COMPLETION_TURNS {
        let validation_tool_required = validation.requires_tool(edit_recovery_required);
        if validation_tool_required {
            validation.mark_model_prompted();
        }
        let repair_required = edit_recovery_required || validation.requires_repair();
        let checkpoint_required =
            tool_checkpoint_pending && !validation_tool_required && !repair_required;

        let mut input = if previous_response_id.is_some() {
            std::mem::take(&mut pending_input)
        } else {
            responses_payload_messages(
                &request.messages,
                final_answer_requested && !validation_tool_required && !repair_required,
                validation_tool_required,
                checkpoint_required,
            )
        };

        if previous_response_id.is_some() {
            if validation_tool_required {
                input.push(responses_user_message(VALIDATION_REQUIRED_INSTRUCTION));
            } else if checkpoint_required {
                input.push(responses_user_message(TOOL_CALL_CHECKPOINT_INSTRUCTION));
            } else if final_answer_requested && !repair_required {
                input.push(responses_user_message(FINAL_ANSWER_INSTRUCTION));
            }
        }

        let code_tools_allowed = !checkpoint_required
            && code_workspace.is_some()
            && (!final_answer_requested || validation_tool_required || repair_required);
        let orchestration_tools_allowed = !checkpoint_required
            && !final_answer_requested
            && request.orchestration_tools_enabled.unwrap_or(false);
        let orchestration_required = request.orchestration_required.unwrap_or(false);
        let any_tools_allowed = code_tools_allowed || orchestration_tools_allowed;
        let mut payload = json!({
            "model": request.model,
            "input": input,
        });

        if let Some(system_prompt) = request.system_prompt.as_deref() {
            let trimmed = system_prompt.trim();
            if !trimmed.is_empty() {
                payload["instructions"] = json!(trimmed);
            }
        }

        if let Some(previous_response_id) = previous_response_id.as_deref() {
            payload["previous_response_id"] = json!(previous_response_id);
        }

        let reasoning_effort = if checkpoint_required {
            Some(TOOL_CALL_CHECKPOINT_REASONING_EFFORT)
        } else {
            request.reasoning_effort.as_deref()
        };

        if let Some(reasoning) = responses_reasoning_payload(reasoning_effort) {
            payload["reasoning"] = reasoning;
        }

        if any_tools_allowed {
            let mut tools: Vec<Value> = Vec::new();
            if code_tools_allowed {
                if let Value::Array(code_tools) = responses_tools_schema(can_write) {
                    tools.extend(code_tools);
                }
            }
            if orchestration_tools_allowed {
                if let Value::Array(orchestration_tools) = responses_orchestration_tools_schema() {
                    tools.extend(orchestration_tools);
                }
            }
            payload["tools"] = Value::Array(tools);
            if repair_required || orchestration_required {
                payload["tool_choice"] = json!("required");
            }
        }

        let parsed = send_responses_request_maybe_stream(
            &app,
            stream_id.as_deref(),
            &client,
            &endpoint,
            &request.api_key,
            &request.provider_name,
            &payload,
            cancellation.as_deref(),
        )
        .await?;

        if let Some(response_id) = responses_id(&parsed) {
            previous_response_id = Some(response_id);
        }

        if let Some(usage) = TraceCtx::usage_from(&parsed) {
            last_usage = Some(usage);
        }

        TraceCtx::append_steps(&mut trace_steps, responses_reasoning_trace_steps(&parsed));
        let tool_calls = responses_function_calls(&parsed);

        if !checkpoint_required {
            if let Some(workspace) = code_workspace.as_ref() {
                if !tool_calls.is_empty() {
                    let remaining_total_tool_budget = MAX_TOTAL_TOOL_CALLS_PER_AUTONOMOUS_ROUND
                        .saturating_sub(total_tool_calls_executed);
                    // Apply the same hard batch cap for Responses API function calls.
                    let tool_execution_plan = ToolCallUtils::plan_execution(
                        tool_calls.len(),
                        tool_calls_since_checkpoint,
                        TOOL_CALL_CHECKPOINT_INTERVAL,
                        max_tool_calls_per_turn,
                        remaining_total_tool_budget,
                    );
                    if tool_execution_plan.executable_count == 0 {
                        if ToolCallUtils::can_open_next_autonomous_round(
                            turn_index,
                            MAX_CHAT_COMPLETION_TURNS,
                        ) {
                            tool_checkpoint_pending = true;
                            tool_budget_reset_pending = true;
                            final_answer_requested = false;
                        } else {
                            tool_checkpoint_pending = false;
                            final_answer_requested = true;
                        }
                        continue;
                    }
                    let executable_tool_calls = ToolCallUtils::truncate_tool_calls(
                        &tool_calls,
                        tool_execution_plan.executable_count,
                    );
                    let mut failed_edit = false;
                    let mut successful_edit = false;
                    let mut validation_run = ValidationRun::default();
                    for response_tool_call in &executable_tool_calls {
                        let tool_call =
                            response_function_call_to_chat_tool_call(response_tool_call);
                        let call_step = tool_call_trace_step(&tool_call);
                        emit_trace_step(&app, stream_id.as_deref(), &call_step);
                        TraceCtx::append_steps(&mut trace_steps, vec![call_step]);

                        let tool_result = if let Some(tool_result) =
                            validation.redundant_validation_tool_result(&tool_call)
                        {
                            tool_result
                        } else {
                            let mut stream_tool_output = |step: ChatTraceStep| {
                                emit_tool_chunk(&app, stream_id.as_deref(), &step);
                            };
                            execute_code_tool_call(
                                workspace,
                                &tool_call,
                                can_write,
                                Some(&mut stream_tool_output),
                            )
                        };

                        validation_run.observe_tool_result(&tool_call, &tool_result);
                        if ValidationOps::result_succeeded(&tool_result)
                            && ValidationOps::is_edit_call(&tool_call)
                        {
                            successful_edit = true;
                        }
                        if ValidationOps::edit_needs_recovery(&tool_call, &tool_result) {
                            failed_edit = true;
                        }

                        let result_step = tool_result_trace_step(&tool_call, &tool_result);
                        emit_trace_step(&app, stream_id.as_deref(), &result_step);
                        TraceCtx::append_steps(&mut trace_steps, vec![result_step]);
                        pending_input.push(response_tool_output(&tool_call, &tool_result));
                    }

                    (edit_recovery_required, edit_recovery_rounds) =
                        ValidationOps::next_recovery_state(
                            edit_recovery_required,
                            edit_recovery_rounds,
                            failed_edit,
                            successful_edit,
                        );
                    if failed_edit && edit_recovery_required {
                        pending_input
                            .push(responses_user_message(EDIT_FAILURE_RECOVERY_INSTRUCTION));
                    }
                    let validation_failed = if successful_edit {
                        validation.mark_successful_edit();
                        false
                    } else {
                        validation.record_run(validation_run)
                    };
                    if validation_failed {
                        pending_input.push(responses_user_message(
                            VALIDATION_FAILURE_RECOVERY_INSTRUCTION,
                        ));
                    }
                    if edit_recovery_required || validation.requires_repair() {
                        final_answer_requested = false;
                    }

                    code_tool_called = true;
                    total_tool_calls_executed += executable_tool_calls.len();
                    ToolCallUtils::schedule_checkpoint(
                        &mut tool_calls_since_checkpoint,
                        &mut tool_checkpoint_pending,
                        executable_tool_calls.len(),
                        TOOL_CALL_CHECKPOINT_INTERVAL,
                    );
                    if is_deepseek && tool_execution_plan.truncated {
                        tool_checkpoint_pending = true;
                    }
                    if total_tool_calls_executed >= MAX_TOTAL_TOOL_CALLS_PER_AUTONOMOUS_ROUND {
                        if ToolCallUtils::can_open_next_autonomous_round(
                            turn_index,
                            MAX_CHAT_COMPLETION_TURNS,
                        ) {
                            tool_checkpoint_pending = true;
                            tool_budget_reset_pending = true;
                            final_answer_requested = false;
                        } else {
                            tool_checkpoint_pending = false;
                            final_answer_requested = true;
                        }
                    }

                    if !validation_tool_required && !repair_required {
                        tool_only_rounds += 1;
                        if !tool_checkpoint_pending && tool_only_rounds >= MAX_TOOL_ONLY_ROUNDS {
                            if ToolCallUtils::can_open_next_autonomous_round(
                                turn_index,
                                MAX_CHAT_COMPLETION_TURNS,
                            ) {
                                tool_checkpoint_pending = true;
                                tool_budget_reset_pending = true;
                                final_answer_requested = false;
                            } else {
                                pending_input
                                    .push(responses_user_message(FINAL_ANSWER_INSTRUCTION));
                                final_answer_requested = true;
                            }
                        }
                    }

                    if validation.should_auto_validate(edit_recovery_required) {
                        validation.mark_auto_attempted();
                        let (validation_outputs, validation_run) =
                            run_default_validation_commands_for_responses(
                                &app,
                                stream_id.as_deref(),
                                workspace,
                                can_write,
                                &mut trace_steps,
                            );

                        if validation_outputs.is_empty() {
                            mark_validation_unavailable_for_responses(&mut pending_input);
                            validation.mark_validator_discovery_required();
                            final_answer_requested = false;
                        } else {
                            pending_input.extend(validation_outputs);
                            if validation.record_run(validation_run) {
                                pending_input.push(responses_user_message(
                                    VALIDATION_FAILURE_RECOVERY_INSTRUCTION,
                                ));
                                final_answer_requested = false;
                            } else {
                                final_answer_requested = false;
                            }
                        }
                    }

                    continue;
                }
            }
        }

        let content = responses_output_text(&parsed);

        if !content.is_empty() {
            if checkpoint_required {
                tool_checkpoint_pending = false;
                final_answer_requested = false;
                if tool_budget_reset_pending {
                    tool_only_rounds = 0;
                    tool_calls_since_checkpoint = 0;
                    total_tool_calls_executed = 0;
                    tool_budget_reset_pending = false;
                }
                continue;
            }
            if validation.is_pending() {
                if let Some(workspace) = code_workspace.as_ref() {
                    if validation.can_auto_validate() {
                        validation.mark_auto_attempted();
                        let (validation_outputs, validation_run) =
                            run_default_validation_commands_for_responses(
                                &app,
                                stream_id.as_deref(),
                                workspace,
                                can_write,
                                &mut trace_steps,
                            );

                        if !validation_outputs.is_empty() {
                            pending_input.extend(validation_outputs);
                            if validation.record_run(validation_run) {
                                pending_input.push(responses_user_message(
                                    VALIDATION_FAILURE_RECOVERY_INSTRUCTION,
                                ));
                                final_answer_requested = false;
                            } else {
                                final_answer_requested = false;
                            }
                            continue;
                        }
                    }
                }

                mark_validation_unavailable_for_responses(&mut pending_input);
                validation.mark_validator_discovery_required();
                final_answer_requested = false;
                continue;
            }

            if responses_output_is_incomplete(&parsed) {
                append_continued_output(&mut accumulated_display_content, &content);
                pending_input.push(responses_user_message(CONTINUE_OUTPUT_INSTRUCTION));
                final_answer_requested = true;
                continue;
            }

            return Ok(ChatCompletionResponse {
                content: combined_output_text(&accumulated_display_content, &content),
                trace_steps,
                usage: last_usage,
                dispatched_tasks: None,
            });
        }

        if checkpoint_required {
            tool_checkpoint_pending = false;
            final_answer_requested = false;
            if tool_budget_reset_pending {
                tool_only_rounds = 0;
                tool_calls_since_checkpoint = 0;
                total_tool_calls_executed = 0;
                tool_budget_reset_pending = false;
            }
            continue;
        }

        if code_tool_called && !final_answer_requested {
            final_answer_requested = true;
            continue;
        }
    }

    Err(format!(
        "{} returned no displayable content from Responses API.",
        request.provider_name
    ))
}

fn is_deepseek_provider(provider_name: &str, base_url: &str) -> bool {
    provider_name.to_ascii_lowercase().contains("deepseek")
        || base_url.to_ascii_lowercase().contains("deepseek")
}

fn reasoning_enabled(reasoning_effort: Option<&str>) -> bool {
    let trimmed = reasoning_effort.unwrap_or("").trim();
    !trimmed.is_empty() && !trimmed.eq_ignore_ascii_case("off")
}

fn apply_reasoning_payload(payload: &mut Value, is_deepseek: bool, reasoning_effort: Option<&str>) {
    let trimmed = reasoning_effort.unwrap_or("").trim();
    let reasoning_enabled = reasoning_enabled(reasoning_effort);

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

fn chat_payload_messages(
    messages: &[Value],
    final_answer_requested: bool,
    validation_required: bool,
    phase_instruction: Option<&str>,
) -> Vec<Value> {
    let mut payload_messages = messages.to_vec();

    if validation_required {
        payload_messages.push(json!({
            "role": "user",
            "content": VALIDATION_REQUIRED_INSTRUCTION,
        }));
    } else if let Some(phase_instruction) = phase_instruction {
        payload_messages.push(json!({
            "role": "user",
            "content": phase_instruction,
        }));
    } else if final_answer_requested {
        payload_messages.push(json!({
            "role": "user",
            "content": FINAL_ANSWER_INSTRUCTION,
        }));
    }

    payload_messages
}

fn responses_user_message(content: &str) -> Value {
    json!({
        "role": "user",
        "content": content,
    })
}

fn mark_validation_unavailable_for_responses(input: &mut Vec<Value>) {
    input.push(responses_user_message(VALIDATION_UNAVAILABLE_INSTRUCTION));
}

fn responses_payload_messages(
    messages: &[ChatMessage],
    final_answer_requested: bool,
    validation_required: bool,
    checkpoint_required: bool,
) -> Vec<Value> {
    let mut payload_messages = messages
        .iter()
        .map(|message| {
            let mut msg = json!({
                "role": message.role,
                "content": message.content,
            });
            if let Some(reasoning) = &message.reasoning_content {
                if !reasoning.trim().is_empty() {
                    msg["reasoning_content"] = json!(reasoning);
                }
            }
            msg
        })
        .collect::<Vec<_>>();

    if validation_required {
        payload_messages.push(responses_user_message(VALIDATION_REQUIRED_INSTRUCTION));
    } else if checkpoint_required {
        payload_messages.push(responses_user_message(TOOL_CALL_CHECKPOINT_INSTRUCTION));
    } else if final_answer_requested {
        payload_messages.push(responses_user_message(FINAL_ANSWER_INSTRUCTION));
    }

    payload_messages
}

fn responses_reasoning_payload(reasoning_effort: Option<&str>) -> Option<Value> {
    if !reasoning_enabled(reasoning_effort) {
        return None;
    }

    Some(json!({
        "effort": reasoning_effort.unwrap_or("").trim(),
        "summary": "auto",
    }))
}

fn responses_tools_schema(allow_writes: bool) -> Value {
    let tools = code_tools_schema(true, allow_writes)
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|tool| {
            let function = tool.get("function")?.clone();
            let mut response_tool = function;
            response_tool["type"] = json!("function");
            Some(response_tool)
        })
        .collect::<Vec<_>>();

    Value::Array(tools)
}

fn responses_orchestration_tools_schema() -> Value {
    let tools = orchestration_tools_schema(true)
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|tool| {
            let function = tool.get("function")?.clone();
            let mut response_tool = function;
            response_tool["type"] = json!("function");
            Some(response_tool)
        })
        .collect::<Vec<_>>();

    Value::Array(tools)
}

fn responses_id(response: &Value) -> Option<String> {
    response
        .get("id")
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn responses_output_text(response: &Value) -> String {
    if let Some(text) = response.get("output_text").and_then(Value::as_str) {
        if !text.trim().is_empty() {
            return text.trim().to_string();
        }
    }

    response
        .get("output")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|item| item.get("type").and_then(Value::as_str) == Some("message"))
        .flat_map(|item| {
            item.get("content")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
        })
        .filter_map(|part| {
            part.get("text")
                .or_else(|| part.get("content"))
                .and_then(Value::as_str)
        })
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

fn responses_reasoning_trace_steps(response: &Value) -> Vec<ChatTraceStep> {
    response
        .get("output")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|item| item.get("type").and_then(Value::as_str) == Some("reasoning"))
        .flat_map(|item| {
            item.get("summary")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
        })
        .filter_map(|summary| {
            summary
                .get("text")
                .or_else(|| summary.get("content"))
                .and_then(Value::as_str)
        })
        .flat_map(TraceCtx::split_trace)
        .map(|line| TraceCtx::trace_step("reasoning", line))
        .collect()
}

fn responses_function_calls(response: &Value) -> Vec<Value> {
    response
        .get("output")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|item| item.get("type").and_then(Value::as_str) == Some("function_call"))
        .cloned()
        .collect()
}

fn response_function_call_to_chat_tool_call(function_call: &Value) -> Value {
    let call_id = function_call
        .get("call_id")
        .or_else(|| function_call.get("id"))
        .and_then(Value::as_str)
        .unwrap_or("responses-tool-call");
    let name = function_call
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let arguments = function_call
        .get("arguments")
        .and_then(Value::as_str)
        .unwrap_or("{}");

    json!({
        "id": call_id,
        "type": "function",
        "function": {
            "name": name,
            "arguments": arguments,
        }
    })
}

fn chat_tool_call_to_response_function_call(tool_call: &Value) -> Value {
    let call_id = tool_call
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or("responses-tool-call");
    let function = tool_call.get("function").unwrap_or(&Value::Null);
    let name = function.get("name").and_then(Value::as_str).unwrap_or("");
    let arguments = function
        .get("arguments")
        .and_then(Value::as_str)
        .unwrap_or("{}");

    json!({
        "type": "function_call",
        "call_id": call_id,
        "name": name,
        "arguments": arguments,
    })
}

fn response_tool_output(tool_call: &Value, tool_result: &Value) -> Value {
    let call_id = tool_call
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or("responses-tool-call");

    json!({
        "type": "function_call_output",
        "call_id": call_id,
        "output": StrUtils::message_content_text(tool_result),
    })
}

fn run_default_validation_commands_for_responses(
    app: &AppHandle,
    stream_id: Option<&str>,
    workspace: &Path,
    can_write: bool,
    trace_steps: &mut Vec<ChatTraceStep>,
) -> (Vec<Value>, ValidationRun) {
    let mut outputs = Vec::new();
    let mut run = ValidationRun::default();

    for tool_call in ValidationOps::make_calls(workspace) {
        let call_step = tool_call_trace_step(&tool_call);
        emit_trace_step(app, stream_id, &call_step);
        TraceCtx::append_steps(trace_steps, vec![call_step]);

        let mut stream_tool_output = |step: ChatTraceStep| {
            emit_tool_chunk(app, stream_id, &step);
        };
        let tool_result = execute_code_tool_call(
            workspace,
            &tool_call,
            can_write,
            Some(&mut stream_tool_output),
        );
        let result_step = tool_result_trace_step(&tool_call, &tool_result);
        emit_trace_step(app, stream_id, &result_step);
        TraceCtx::append_steps(trace_steps, vec![result_step]);
        run.observe_tool_result(&tool_call, &tool_result);

        outputs.push(chat_tool_call_to_response_function_call(&tool_call));
        outputs.push(response_tool_output(&tool_call, &tool_result));
    }

    (outputs, run)
}

fn first_choice_finish_reason(parsed: &Value) -> Option<String> {
    parsed
        .get("choices")
        .and_then(|choices| choices.get(0))
        .and_then(|choice| choice.get("finish_reason"))
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn finish_reason_indicates_truncated_output(reason: Option<&str>) -> bool {
    reason.is_some_and(|value| {
        matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "length" | "max_tokens"
        )
    })
}

fn responses_output_is_incomplete(response: &Value) -> bool {
    response
        .get("status")
        .and_then(Value::as_str)
        .is_some_and(|status| status.eq_ignore_ascii_case("incomplete"))
        || response.get("incomplete_details").is_some()
}

fn append_continued_output(accumulated: &mut String, next: &str) {
    let trimmed = next.trim();
    if trimmed.is_empty() {
        return;
    }

    if !accumulated.is_empty() {
        accumulated.push('\n');
        accumulated.push('\n');
    }

    accumulated.push_str(trimmed);
}

fn combined_output_text(accumulated: &str, next: &str) -> String {
    let trimmed_next = next.trim();
    if accumulated.is_empty() {
        return trimmed_next.to_string();
    }
    if trimmed_next.is_empty() {
        return accumulated.to_string();
    }

    format!("{}\n\n{}", accumulated, trimmed_next)
}

fn should_retry_http_failure(status: reqwest::StatusCode, _body: &str) -> bool {
    status.is_server_error()
        || matches!(
            status,
            reqwest::StatusCode::REQUEST_TIMEOUT
                | reqwest::StatusCode::TOO_MANY_REQUESTS
                | reqwest::StatusCode::CONFLICT
        )
}

fn request_was_cancelled(cancellation: Option<&AtomicBool>) -> bool {
    cancellation.is_some_and(|token| token.load(Ordering::Acquire))
}

async fn wait_for_http_retry(
    delay: Duration,
    cancellation: Option<&AtomicBool>,
) -> Result<(), String> {
    let deadline = Instant::now() + delay;

    loop {
        if request_was_cancelled(cancellation) {
            return Err("Chat completion was cancelled.".to_string());
        }

        let now = Instant::now();
        if now >= deadline {
            return Ok(());
        }

        tokio::time::sleep((deadline - now).min(RETRY_CANCELLATION_POLL_INTERVAL)).await;
    }
}

async fn send_http_request_with_retry(
    client: &reqwest::Client,
    endpoint: &str,
    api_key: &str,
    provider_name: &str,
    payload: &Value,
    cancellation: Option<&AtomicBool>,
    retry_progress: Option<&(dyn Fn(HttpRetryProgress) + Sync)>,
    initial_retry_delay: Duration,
) -> Result<reqwest::Response, String> {
    let retry_delay = initial_retry_delay;
    let mut retry_attempt = 0;

    loop {
        if request_was_cancelled(cancellation) {
            return Err("Chat completion was cancelled.".to_string());
        }

        let response = match client
            .post(endpoint)
            .bearer_auth(api_key.trim())
            .json(payload)
            .send()
            .await
        {
            Ok(response) => response,
            Err(error) => {
                retry_attempt += 1;
                if let Some(notify) = retry_progress {
                    notify(HttpRetryProgress::Waiting {
                        attempt: retry_attempt,
                        delay: retry_delay,
                        reason: format!("Request to {} failed: {}", provider_name, error),
                    });
                }
                wait_for_http_retry(retry_delay, cancellation).await?;
                continue;
            }
        };
        let status = response.status();

        if status.is_success() {
            if retry_attempt > 0 {
                if let Some(notify) = retry_progress {
                    notify(HttpRetryProgress::Recovered {
                        attempts: retry_attempt,
                    });
                }
            }
            return Ok(response);
        }

        let body = match response.text().await {
            Ok(body) => body,
            Err(error) => {
                retry_attempt += 1;
                if let Some(notify) = retry_progress {
                    notify(HttpRetryProgress::Waiting {
                        attempt: retry_attempt,
                        delay: retry_delay,
                        reason: format!("Failed to read {} response: {}", provider_name, error),
                    });
                }
                wait_for_http_retry(retry_delay, cancellation).await?;
                continue;
            }
        };

        if !should_retry_http_failure(status, &body) {
            return Err(format!(
                "{} returned HTTP {}: {}",
                provider_name, status, body
            ));
        }

        retry_attempt += 1;
        if let Some(notify) = retry_progress {
            notify(HttpRetryProgress::Waiting {
                attempt: retry_attempt,
                delay: retry_delay,
                reason: format!("{} returned HTTP {}: {}", provider_name, status, body),
            });
        }
        wait_for_http_retry(retry_delay, cancellation).await?;
    }
}

async fn send_chat_completion_request(
    client: &reqwest::Client,
    endpoint: &str,
    api_key: &str,
    provider_name: &str,
    payload: &Value,
    cancellation: Option<&AtomicBool>,
) -> Result<Value, String> {
    let response = send_http_request_with_retry(
        client,
        endpoint,
        api_key,
        provider_name,
        payload,
        cancellation,
        None,
        HTTP_RETRY_DELAY,
    )
    .await?;
    let body = response
        .text()
        .await
        .map_err(|error| format!("Failed to read {} response: {}", provider_name, error))?;

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
    cancellation: Option<&AtomicBool>,
    suppress_content_stream: bool,
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
            cancellation,
            suppress_content_stream,
        )
        .await
    } else {
        send_chat_completion_request(
            client,
            endpoint,
            api_key,
            provider_name,
            payload,
            cancellation,
        )
        .await
    }
}

async fn send_responses_request(
    client: &reqwest::Client,
    endpoint: &str,
    api_key: &str,
    provider_name: &str,
    payload: &Value,
    cancellation: Option<&AtomicBool>,
) -> Result<Value, String> {
    let response = send_http_request_with_retry(
        client,
        endpoint,
        api_key,
        provider_name,
        payload,
        cancellation,
        None,
        HTTP_RETRY_DELAY,
    )
    .await?;
    let body = response
        .text()
        .await
        .map_err(|error| format!("Failed to read {} response: {}", provider_name, error))?;

    serde_json::from_str(&body)
        .map_err(|error| format!("Failed to parse {} response: {}", provider_name, error))
}

async fn send_responses_request_maybe_stream(
    app: &AppHandle,
    stream_id: Option<&str>,
    client: &reqwest::Client,
    endpoint: &str,
    api_key: &str,
    provider_name: &str,
    payload: &Value,
    cancellation: Option<&AtomicBool>,
) -> Result<Value, String> {
    if let Some(stream_id) = stream_id {
        send_responses_stream_request(
            app,
            stream_id,
            client,
            endpoint,
            api_key,
            provider_name,
            payload,
            cancellation,
        )
        .await
    } else {
        send_responses_request(
            client,
            endpoint,
            api_key,
            provider_name,
            payload,
            cancellation,
        )
        .await
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
            retry_attempt: None,
            retry_delay_ms: None,
            retry_reason: None,
        },
    );
}

fn emit_overload_retry_event(app: &AppHandle, stream_id: &str, progress: HttpRetryProgress) {
    let (event_type, retry_attempt, retry_delay_ms, retry_reason) = match progress {
        HttpRetryProgress::Waiting {
            attempt,
            delay,
            reason,
        } => (
            "retryWaiting",
            attempt,
            Some(delay.as_millis().min(u64::MAX as u128) as u64),
            Some(reason),
        ),
        HttpRetryProgress::Recovered { attempts } => ("retryRecovered", attempts, None, None),
    };

    let _ = app.emit(
        CHAT_COMPLETION_STREAM_EVENT,
        ChatCompletionStreamEvent {
            stream_id: stream_id.to_string(),
            event_type: event_type.to_string(),
            trace_kind: None,
            text: String::new(),
            detail: None,
            usage: None,
            retry_attempt: Some(retry_attempt),
            retry_delay_ms,
            retry_reason,
        },
    );
}

/// If tool_call is a successful dispatch_tasks invocation, return the task entries.
fn extract_dispatched_tasks(tool_call: &Value, tool_result: &Value) -> Option<Vec<TaskDispatchedEntry>> {
    let function = tool_call.get("function")?;
    let name = function.get("name")?.as_str()?;
    if name != "dispatch_tasks" {
        return None;
    }

    if !ValidationOps::result_succeeded(tool_result) {
        return None;
    }

    let arguments: Value = function
        .get("arguments")
        .and_then(|a| serde_json::from_str(a.as_str()?).ok())?;
    let tasks = arguments.get("tasks")?.as_array()?;

    let entries: Vec<TaskDispatchedEntry> = tasks
        .iter()
        .filter_map(|task| {
            let member = task
                .get("member")?
                .as_str()?
                .trim()
                .trim_start_matches('@')
                .to_string();
            let instruction = task.get("instruction")?.as_str()?.trim().to_string();
            if member.is_empty() || instruction.is_empty() {
                return None;
            }
            Some(TaskDispatchedEntry { member, instruction })
        })
        .collect();

    if entries.is_empty() { None } else { Some(entries) }
}

fn should_finish_after_dispatch_tasks(
    dispatched_tasks: Option<&[TaskDispatchedEntry]>,
    validation: &ValidationState,
    edit_recovery_required: bool,
) -> bool {
    dispatched_tasks.is_some()
        && !edit_recovery_required
        && !validation.requires_tool(edit_recovery_required)
        && !validation.requires_repair()
}

fn dispatched_tasks_completion_content(entries: &[TaskDispatchedEntry]) -> String {
    entries
        .iter()
        .map(|entry| format!("- @{}: {}", entry.member, entry.instruction))
        .collect::<Vec<_>>()
        .join("\n")
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
    suppress_content_stream: bool,
) {
    if let Some(next_usage) = TraceCtx::usage_from(parsed) {
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
                if !suppress_content_stream {
                    emit_content_chunk(app, stream_id, chunk);
                }
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
    cancellation: Option<&AtomicBool>,
    suppress_content_stream: bool,
) -> Result<Value, String> {
    let mut payload = payload.clone();
    payload["stream"] = json!(true);
    payload["stream_options"] = json!({ "include_usage": true });
    let report_retry = |progress| emit_overload_retry_event(app, stream_id, progress);

    let response = send_http_request_with_retry(
        client,
        endpoint,
        api_key,
        provider_name,
        &payload,
        cancellation,
        Some(&report_retry),
        HTTP_RETRY_DELAY,
    )
    .await?;

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
                    suppress_content_stream,
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
                suppress_content_stream,
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

fn response_stream_error_text(parsed: &Value) -> Option<String> {
    let error = parsed
        .get("response")
        .and_then(|response| response.get("error"))
        .or_else(|| parsed.get("error"))?;

    error
        .get("message")
        .and_then(Value::as_str)
        .or_else(|| error.as_str())
        .map(str::to_string)
}

fn collect_response_reasoning_summary(item: &Value) -> String {
    item.get("summary")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|summary| {
            summary
                .get("text")
                .or_else(|| summary.get("content"))
                .and_then(Value::as_str)
        })
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

fn response_stream_text(parsed: &Value) -> Option<&str> {
    parsed
        .get("delta")
        .and_then(Value::as_str)
        .or_else(|| parsed.get("text").and_then(Value::as_str))
        .or_else(|| {
            parsed.get("part").and_then(|part| {
                part.get("text")
                    .or_else(|| part.get("content"))
                    .and_then(Value::as_str)
            })
        })
}

fn append_response_reasoning(
    reasoning: &mut String,
    next_text: &str,
    separate_complete_fragment: bool,
) -> Option<String> {
    if next_text.is_empty() || reasoning.ends_with(next_text) {
        return None;
    }

    if !reasoning.is_empty() && next_text.starts_with(reasoning.as_str()) {
        let delta = next_text[reasoning.len()..].to_string();

        if delta.is_empty() {
            return None;
        }

        reasoning.push_str(&delta);
        return Some(delta);
    }

    let mut emitted = String::new();

    if separate_complete_fragment
        && !reasoning.is_empty()
        && !reasoning.ends_with('\n')
        && !next_text.starts_with('\n')
    {
        reasoning.push('\n');
        emitted.push('\n');
    }

    reasoning.push_str(next_text);
    emitted.push_str(next_text);
    Some(emitted)
}

fn apply_responses_stream_event(
    app: &AppHandle,
    stream_id: &str,
    parsed: &Value,
    content: &mut String,
    reasoning: &mut String,
    function_calls: &mut Vec<Value>,
    response_id: &mut Option<String>,
    usage: &mut Option<ChatCompletionUsage>,
    completed_response: &mut Option<Value>,
) -> Option<String> {
    if let Some(id) = parsed
        .get("response")
        .and_then(|response| response.get("id"))
        .and_then(Value::as_str)
        .or_else(|| parsed.get("id").and_then(Value::as_str))
    {
        *response_id = Some(id.to_string());
    }

    if let Some(response) = parsed.get("response") {
        if let Some(next_usage) = TraceCtx::usage_from(response) {
            emit_usage_event(app, stream_id, next_usage.clone());
            *usage = Some(next_usage);
        }
    }

    let event_type = parsed.get("type").and_then(Value::as_str).unwrap_or("");

    match event_type {
        "response.output_text.delta" => {
            if let Some(delta) = parsed.get("delta").and_then(Value::as_str) {
                if !delta.is_empty() {
                    content.push_str(delta);
                    emit_content_chunk(app, stream_id, delta);
                }
            }
        }
        "response.reasoning_summary_text.delta" | "response.reasoning_text.delta" => {
            if let Some(delta) = response_stream_text(parsed) {
                if let Some(emitted) = append_response_reasoning(reasoning, delta, false) {
                    emit_trace_chunk(app, stream_id, "reasoning", &emitted);
                }
            }
        }
        "response.reasoning_summary_part.added"
        | "response.reasoning_summary_part.done"
        | "response.reasoning_summary_text.done"
        | "response.reasoning_text.done" => {
            if let Some(text) = response_stream_text(parsed) {
                if let Some(emitted) = append_response_reasoning(reasoning, text, true) {
                    emit_trace_chunk(app, stream_id, "reasoning", &emitted);
                }
            }
        }
        "response.output_item.done" => {
            if let Some(item) = parsed.get("item") {
                if item.get("type").and_then(Value::as_str) == Some("function_call") {
                    function_calls.push(item.clone());
                } else if item.get("type").and_then(Value::as_str) == Some("reasoning") {
                    let summary = collect_response_reasoning_summary(item);
                    if !summary.is_empty() {
                        if let Some(emitted) = append_response_reasoning(reasoning, &summary, true)
                        {
                            emit_trace_chunk(app, stream_id, "reasoning", &emitted);
                        }
                    }
                }
            }
        }
        "response.completed" => {
            if let Some(response) = parsed.get("response") {
                if let Some(next_usage) = TraceCtx::usage_from(response) {
                    emit_usage_event(app, stream_id, next_usage.clone());
                    *usage = Some(next_usage);
                }
                *completed_response = Some(response.clone());
            }
        }
        "response.failed" | "response.incomplete" => {
            return response_stream_error_text(parsed);
        }
        _ => {}
    }

    None
}

async fn send_responses_stream_request(
    app: &AppHandle,
    stream_id: &str,
    client: &reqwest::Client,
    endpoint: &str,
    api_key: &str,
    provider_name: &str,
    payload: &Value,
    cancellation: Option<&AtomicBool>,
) -> Result<Value, String> {
    let mut payload = payload.clone();
    payload["stream"] = json!(true);
    let report_retry = |progress| emit_overload_retry_event(app, stream_id, progress);

    let response = send_http_request_with_retry(
        client,
        endpoint,
        api_key,
        provider_name,
        &payload,
        cancellation,
        Some(&report_retry),
        HTTP_RETRY_DELAY,
    )
    .await?;

    let mut content = String::new();
    let mut reasoning = String::new();
    let mut function_calls = Vec::new();
    let mut response_id: Option<String> = None;
    let mut usage: Option<ChatCompletionUsage> = None;
    let mut completed_response: Option<Value> = None;
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

                if let Some(error) = apply_responses_stream_event(
                    app,
                    stream_id,
                    &parsed,
                    &mut content,
                    &mut reasoning,
                    &mut function_calls,
                    &mut response_id,
                    &mut usage,
                    &mut completed_response,
                ) {
                    return Err(format!("{} response failed: {}", provider_name, error));
                }
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

            if let Some(error) = apply_responses_stream_event(
                app,
                stream_id,
                &parsed,
                &mut content,
                &mut reasoning,
                &mut function_calls,
                &mut response_id,
                &mut usage,
                &mut completed_response,
            ) {
                return Err(format!("{} response failed: {}", provider_name, error));
            }
        }
    }

    if let Some(response) = completed_response {
        return Ok(response);
    }

    let mut output = Vec::new();
    if !reasoning.trim().is_empty() {
        output.push(json!({
            "type": "reasoning",
            "summary": [{ "type": "summary_text", "text": reasoning }],
        }));
    }
    if !content.trim().is_empty() {
        output.push(json!({
            "type": "message",
            "content": [{ "type": "output_text", "text": content }],
        }));
    }
    output.extend(function_calls);

    Ok(json!({
        "id": response_id,
        "output": output,
        "output_text": content,
        "usage": usage,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

    fn spawn_status_then_success_server(
        retry_status: &'static str,
        retry_body: &'static str,
        retry_count: usize,
    ) -> (String, thread::JoinHandle<usize>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("test server should bind");
        let address = listener
            .local_addr()
            .expect("test server should have an address");
        let handle = thread::spawn(move || {
            let mut requests = 0;

            for attempt in 0..=retry_count {
                let (mut stream, _) = listener.accept().expect("test server should accept");
                let mut request = [0_u8; 4096];
                let _ = stream
                    .read(&mut request)
                    .expect("test server should read request");
                requests += 1;

                let (status, body) = if attempt < retry_count {
                    (retry_status, retry_body)
                } else {
                    ("200 OK", r#"{"output_text":"recovered"}"#)
                };
                let response = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                stream
                    .write_all(response.as_bytes())
                    .expect("test server should write response");
            }

            requests
        });

        (format!("http://{address}/v1/responses"), handle)
    }

    fn spawn_overload_then_success_server(
        overload_count: usize,
    ) -> (String, thread::JoinHandle<usize>) {
        spawn_status_then_success_server(
            "503 Service Unavailable",
            r#"{"error":{"message":"system cpu overloaded (current: 93.1%, threshold: 85%)","type":"new_api_error","param":"","code":"system_cpu_overloaded"}}"#,
            overload_count,
        )
    }

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
    fn chat_completion_turn_guard_allows_multiple_tool_rounds() {
        assert!(MAX_CHAT_COMPLETION_TURNS >= 16);
    }

    #[test]
    fn http_retry_delay_is_fixed_at_five_seconds() {
        assert_eq!(HTTP_RETRY_DELAY, Duration::from_secs(5));
    }

    #[test]
    fn retries_new_api_resource_overload_until_request_succeeds() {
        let (endpoint, server) = spawn_overload_then_success_server(2);
        let progress = Mutex::new(Vec::new());
        let record_progress = |event| {
            progress
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push(event)
        };
        let response = tauri::async_runtime::block_on(send_http_request_with_retry(
            &reqwest::Client::new(),
            &endpoint,
            "test-key",
            "ChatGPT",
            &json!({ "model": "gpt-5.5", "input": "test" }),
            None,
            Some(&record_progress),
            std::time::Duration::ZERO,
        ))
        .expect("resource overload should be retried");
        let body: Value =
            tauri::async_runtime::block_on(response.json()).expect("response should parse");

        assert_eq!(body["output_text"], "recovered");
        assert_eq!(server.join().expect("test server should finish"), 3);
        assert_eq!(
            progress
                .into_inner()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
            vec![
                HttpRetryProgress::Waiting {
                    attempt: 1,
                    delay: Duration::ZERO,
                    reason: r#"ChatGPT returned HTTP 503 Service Unavailable: {"error":{"message":"system cpu overloaded (current: 93.1%, threshold: 85%)","type":"new_api_error","param":"","code":"system_cpu_overloaded"}}"#.to_string(),
                },
                HttpRetryProgress::Waiting {
                    attempt: 2,
                    delay: Duration::ZERO,
                    reason: r#"ChatGPT returned HTTP 503 Service Unavailable: {"error":{"message":"system cpu overloaded (current: 93.1%, threshold: 85%)","type":"new_api_error","param":"","code":"system_cpu_overloaded"}}"#.to_string(),
                },
                HttpRetryProgress::Recovered { attempts: 2 },
            ]
        );
    }

    #[test]
    fn retries_bad_gateway_until_request_succeeds() {
        let (endpoint, server) =
            spawn_status_then_success_server("502 Bad Gateway", "error code: 502", 1);
        let progress = Mutex::new(Vec::new());
        let record_progress = |event| {
            progress
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push(event)
        };
        let response = tauri::async_runtime::block_on(send_http_request_with_retry(
            &reqwest::Client::new(),
            &endpoint,
            "test-key",
            "ChatGPT",
            &json!({ "model": "gpt-5.5", "input": "test" }),
            None,
            Some(&record_progress),
            std::time::Duration::ZERO,
        ))
        .expect("bad gateway should be retried");
        let body: Value =
            tauri::async_runtime::block_on(response.json()).expect("response should parse");

        assert_eq!(body["output_text"], "recovered");
        assert_eq!(server.join().expect("test server should finish"), 2);
        assert_eq!(
            progress
                .into_inner()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
            vec![
                HttpRetryProgress::Waiting {
                    attempt: 1,
                    delay: Duration::ZERO,
                    reason: "ChatGPT returned HTTP 502 Bad Gateway: error code: 502".to_string(),
                },
                HttpRetryProgress::Recovered { attempts: 1 },
            ]
        );
    }

    #[test]
    fn retries_only_transient_http_failures() {
        assert!(!should_retry_http_failure(
            reqwest::StatusCode::BAD_REQUEST,
            "bad request",
        ));
        assert!(should_retry_http_failure(
            reqwest::StatusCode::TOO_MANY_REQUESTS,
            "rate limited",
        ));
        assert!(should_retry_http_failure(
            reqwest::StatusCode::BAD_GATEWAY,
            "bad gateway",
        ));
        assert!(should_retry_http_failure(
            reqwest::StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
        ));
        assert!(!should_retry_http_failure(reqwest::StatusCode::OK, "{}"));
    }

    #[test]
    fn cancelled_retry_stops_before_sending_another_request() {
        let cancellation = AtomicBool::new(true);
        let error = tauri::async_runtime::block_on(send_http_request_with_retry(
            &reqwest::Client::new(),
            "http://127.0.0.1:1/v1/responses",
            "test-key",
            "ChatGPT",
            &json!({ "model": "gpt-5.5", "input": "test" }),
            Some(&cancellation),
            None,
            Duration::ZERO,
        ))
        .expect_err("cancelled request should stop");

        assert_eq!(error, "Chat completion was cancelled.");
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
            StrUtils::strip_ansi_escape_sequences(
                "\x1b[1mCodeGraph Status\x1b[0m\n\x1b[32m[OK]\x1b[0m"
            ),
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
    fn chat_message_deserializes_reasoning_content_from_camel_case() {
        let message: ChatMessage = serde_json::from_value(json!({
            "role": "assistant",
            "content": "done",
            "reasoningContent": "step one"
        }))
        .expect("camelCase reasoningContent should deserialize");

        assert_eq!(message.reasoning_content.as_deref(), Some("step one"));
    }

    #[test]
    fn chat_message_deserializes_reasoning_content_from_snake_case_alias() {
        let message: ChatMessage = serde_json::from_value(json!({
            "role": "assistant",
            "content": "done",
            "reasoning_content": "step one"
        }))
        .expect("snake_case reasoning_content should deserialize");

        assert_eq!(message.reasoning_content.as_deref(), Some("step one"));
    }

    #[test]
    fn failed_edit_tool_result_requests_recovery() {
        let tool_call = json!({
            "function": {
                "name": "apply_patch",
                "arguments": "{\"patchText\":\"diff --git a/a b/a\"}"
            }
        });
        let failed_result = json!({
            "role": "tool",
            "content": "Tool apply_patch failed: git apply failed: patch does not apply"
        });
        let successful_result = json!({
            "role": "tool",
            "content": "Patch applied to files:\na"
        });

        assert!(ValidationOps::edit_needs_recovery(
            &tool_call,
            &failed_result
        ));
        assert!(!ValidationOps::edit_needs_recovery(
            &tool_call,
            &successful_result
        ));
    }

    #[test]
    fn edit_recovery_forces_the_next_supported_tool_call() {
        let state = AgentTurnState::new(false, false, false);
        assert_eq!(
            state.tool_choice(AgentTurnPhase::Conversation, false, true, false),
            Some(json!("required"))
        );
    }

    #[test]
    fn edit_recovery_persists_through_reads_and_clears_after_a_successful_edit() {
        assert_eq!(
            ValidationOps::next_recovery_state(false, 0, true, false),
            (true, 0)
        );
        assert_eq!(
            ValidationOps::next_recovery_state(true, 0, false, false),
            (true, 1)
        );
        assert_eq!(
            ValidationOps::next_recovery_state(true, 1, false, true),
            (false, 0)
        );
        assert_eq!(
            ValidationOps::next_recovery_state(
                true,
                MAX_EDIT_RECOVERY_TOOL_ROUNDS - 1,
                false,
                false,
            ),
            (false, MAX_EDIT_RECOVERY_TOOL_ROUNDS)
        );
    }

    #[test]
    fn edit_recovery_takes_priority_over_validation() {
        let mut validation = ValidationState::default();
        validation.mark_successful_edit();

        assert!(!validation.requires_tool(true));
        assert!(validation.requires_tool(false));
    }

    #[test]
    fn validation_instructions_require_repair_instead_of_early_final_answer() {
        let required = VALIDATION_REQUIRED_INSTRUCTION.to_ascii_lowercase();
        let unavailable = VALIDATION_UNAVAILABLE_INSTRUCTION.to_ascii_lowercase();

        assert!(required.contains("fix"));
        assert!(required.contains("until it passes"));
        assert!(!VALIDATION_UNAVAILABLE_INSTRUCTION.contains("Write the final answer now"));
        assert!(unavailable.contains("inspect"));
    }

    #[test]
    fn openai_reasoning_models_use_responses_api() {
        let request = ChatCompletionRequest {
            provider_name: "ChatGPT".to_string(),
            base_url: "http://127.0.0.1:15721/codex/v1".to_string(),
            api_key: "test".to_string(),
            model: "gpt-5.5".to_string(),
            wire_api: None,
            reasoning_effort: Some("high".to_string()),
            temperature: Some(0.7),
            system_prompt: None,
            workspace_path: None,
            code_tools_enabled: None,
            orchestration_tools_enabled: None,
            orchestration_required: None,
            can_write: None,
            stream_id: None,
            cancellation_id: None,
            messages: vec![],
        };

        assert!(should_use_responses_api(&request, false));
        assert!(!should_use_responses_api(&request, true));

        let mut chat_wire_request = request;
        chat_wire_request.base_url = "https://relay.example.com/v1".to_string();
        chat_wire_request.wire_api = Some("chat".to_string());
        assert!(!should_use_responses_api(&chat_wire_request, false));

        let mut unknown_proxy_request = chat_wire_request;
        unknown_proxy_request.wire_api = None;
        assert!(!should_use_responses_api(&unknown_proxy_request, false));
    }

    #[test]
    fn responses_reasoning_payload_requests_summary_when_enabled() {
        assert_eq!(
            responses_reasoning_payload(Some("medium")),
            Some(json!({ "effort": "medium", "summary": "auto" }))
        );
        assert_eq!(responses_reasoning_payload(Some("off")), None);
    }

    #[test]
    fn responses_endpoint_accepts_base_chat_or_full_endpoint() {
        assert_eq!(
            responses_endpoint("https://api.example.com/v1"),
            "https://api.example.com/v1/responses"
        );
        assert_eq!(
            responses_endpoint("https://api.example.com/v1/chat/completions"),
            "https://api.example.com/v1/responses"
        );
        assert_eq!(
            responses_endpoint("https://api.example.com/v1/responses"),
            "https://api.example.com/v1/responses"
        );
    }

    #[test]
    fn parses_responses_text_reasoning_and_usage() {
        let response = json!({
            "id": "resp_123",
            "output_text": "final answer",
            "output": [
                {
                    "type": "reasoning",
                    "summary": [
                        { "type": "summary_text", "text": "checked the workspace" }
                    ]
                }
            ],
            "usage": {
                "input_tokens": 100,
                "output_tokens": 25,
                "total_tokens": 125,
                "input_tokens_details": { "cached_tokens": 40 }
            }
        });

        assert_eq!(responses_id(&response).as_deref(), Some("resp_123"));
        assert_eq!(responses_output_text(&response), "final answer");
        assert_eq!(
            responses_reasoning_trace_steps(&response)[0].text,
            "checked the workspace"
        );

        let usage = TraceCtx::usage_from(&response).unwrap();
        assert_eq!(usage.prompt_tokens, Some(100));
        assert_eq!(usage.completion_tokens, Some(25));
        assert_eq!(usage.prompt_cache_hit_tokens, Some(40));
        assert_eq!(usage.prompt_cache_miss_tokens, Some(60));
    }

    #[test]
    fn appends_responses_reasoning_stream_without_done_duplicates() {
        let mut reasoning = String::new();

        assert_eq!(
            append_response_reasoning(&mut reasoning, "checked", false).as_deref(),
            Some("checked")
        );
        assert_eq!(
            append_response_reasoning(&mut reasoning, "checked", true),
            None
        );
        assert_eq!(
            append_response_reasoning(&mut reasoning, "checked workspace", true).as_deref(),
            Some(" workspace")
        );
        assert_eq!(
            append_response_reasoning(&mut reasoning, "opened files", true).as_deref(),
            Some("\nopened files")
        );
        assert_eq!(reasoning, "checked workspace\nopened files");
    }

    #[test]
    fn extracts_responses_reasoning_stream_text_shapes() {
        assert_eq!(response_stream_text(&json!({ "delta": "a" })), Some("a"));
        assert_eq!(response_stream_text(&json!({ "text": "b" })), Some("b"));
        assert_eq!(
            response_stream_text(&json!({ "part": { "type": "summary_text", "text": "c" } })),
            Some("c")
        );
    }

    #[test]
    fn converts_responses_function_calls_to_chat_tools_and_outputs() {
        let response_call = json!({
            "type": "function_call",
            "call_id": "call_123",
            "name": "read_file",
            "arguments": "{\"file\":\"src/lib.rs\"}"
        });
        let tool_call = response_function_call_to_chat_tool_call(&response_call);

        assert_eq!(tool_call["id"], json!("call_123"));
        assert_eq!(tool_call["function"]["name"], json!("read_file"));

        let tool_output = response_tool_output(
            &tool_call,
            &json!({
                "role": "tool",
                "tool_call_id": "call_123",
                "content": "file contents"
            }),
        );

        assert_eq!(tool_output["type"], json!("function_call_output"));
        assert_eq!(tool_output["call_id"], json!("call_123"));
        assert_eq!(tool_output["output"], json!("file contents"));
    }

    #[test]
    fn final_answer_request_appends_internal_instruction() {
        let messages = vec![json!({ "role": "user", "content": "question" })];
        let payload_messages = chat_payload_messages(&messages, true, false, None);

        assert_eq!(payload_messages.len(), 2);
        assert_eq!(payload_messages[0]["content"], json!("question"));
        assert_eq!(payload_messages[1]["role"], json!("user"));
        assert!(payload_messages[1]["content"]
            .as_str()
            .unwrap()
            .contains("final answer"));
    }

    #[test]
    fn validation_request_takes_priority_over_final_answer_instruction() {
        let messages = vec![json!({ "role": "user", "content": "question" })];
        let payload_messages = chat_payload_messages(&messages, true, true, None);

        assert_eq!(payload_messages.len(), 2);
        assert!(payload_messages[1]["content"]
            .as_str()
            .unwrap()
            .to_ascii_lowercase()
            .contains("call run_command"));
        assert!(!payload_messages[1]["content"]
            .as_str()
            .unwrap()
            .contains(FINAL_ANSWER_INSTRUCTION));
    }

    #[test]
    fn checkpoint_request_takes_priority_over_final_answer_instruction() {
        let messages = vec![json!({ "role": "user", "content": "question" })];
        let payload_messages = chat_payload_messages(
            &messages,
            true,
            false,
            Some(TOOL_CALL_CHECKPOINT_INSTRUCTION),
        );

        assert_eq!(payload_messages.len(), 2);
        assert!(payload_messages[1]["content"]
            .as_str()
            .unwrap()
            .contains("do not call any tools"));
        assert!(!payload_messages[1]["content"]
            .as_str()
            .unwrap()
            .contains(FINAL_ANSWER_INSTRUCTION));
    }

    #[test]
    fn tool_action_request_appends_internal_instruction() {
        let messages = vec![json!({ "role": "user", "content": "question" })];
        let payload_messages = chat_payload_messages(
            &messages,
            false,
            false,
            Some(DEEPSEEK_TOOL_ACTION_INSTRUCTION),
        );

        assert_eq!(payload_messages.len(), 2);
        assert_eq!(payload_messages[0]["content"], json!("question"));
        assert_eq!(payload_messages[1]["role"], json!("user"));
        assert_eq!(
            payload_messages[1]["content"],
            json!(DEEPSEEK_TOOL_ACTION_INSTRUCTION)
        );
    }

    #[test]
    fn dispatched_tasks_completion_content_lists_assignments() {
        let entries = vec![
            TaskDispatchedEntry {
                member: "Silver Wolf".to_string(),
                instruction: "Inspect the log loop".to_string(),
            },
            TaskDispatchedEntry {
                member: "Kafka".to_string(),
                instruction: "Summarize the worker findings".to_string(),
            },
        ];

        assert_eq!(
            dispatched_tasks_completion_content(&entries),
            "- @Silver Wolf: Inspect the log loop\n- @Kafka: Summarize the worker findings"
        );
    }

    #[test]
    fn dispatch_tasks_can_finish_without_reflection_when_validation_is_clear() {
        let validation = ValidationState::default();
        let entries = vec![TaskDispatchedEntry {
            member: "Silver Wolf".to_string(),
            instruction: "Inspect the log loop".to_string(),
        }];

        assert!(should_finish_after_dispatch_tasks(
            Some(entries.as_slice()),
            &validation,
            false
        ));

        let mut pending_validation = ValidationState::default();
        pending_validation.mark_successful_edit();
        assert!(!should_finish_after_dispatch_tasks(
            Some(entries.as_slice()),
            &pending_validation,
            false
        ));

        assert!(!should_finish_after_dispatch_tasks(
            Some(entries.as_slice()),
            &validation,
            true
        ));
    }

    #[test]
    fn extracts_message_content_from_string_and_text_parts() {
        assert_eq!(
            StrUtils::message_content_text(&json!({ "content": "  hello  " })),
            "hello"
        );
        assert_eq!(
            StrUtils::message_content_text(&json!({
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
            TraceCtx::message_reasoning(&json!({
                "reasoning_content": "  line one\nline two  ",
                "content": "answer"
            })),
            "line one\nline two"
        );
    }

    #[test]
    fn extracts_deepseek_prompt_cache_usage() {
        let usage = TraceCtx::usage_from(&json!({
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
        assert!(names.contains(&"codegraph_command"));
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

        let apply_patch_tool = schema
            .as_array()
            .unwrap()
            .iter()
            .find(|tool| {
                tool.get("function")
                    .and_then(|function| function.get("name"))
                    .and_then(Value::as_str)
                    == Some("apply_patch")
            })
            .expect("apply_patch tool should be present");
        let apply_patch_description = apply_patch_tool["function"]["description"]
            .as_str()
            .expect("apply_patch should describe safe patch construction");
        assert!(apply_patch_description.contains("read the exact current target location"));
        assert!(apply_patch_description.contains("hand-guessed line numbers"));
        assert!(apply_patch_description.contains("checkOnly=true"));
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
        assert!(names.contains(&"codegraph_command"));
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

        let codegraph_commands = schema
            .as_array()
            .unwrap()
            .iter()
            .find(|tool| tool["function"]["name"] == json!("codegraph_command"))
            .and_then(|tool| {
                tool["function"]["parameters"]["properties"]["command"]["enum"].as_array()
            })
            .unwrap();
        assert!(codegraph_commands.contains(&json!("status")));
        assert!(!codegraph_commands.contains(&json!("sync")));
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
    fn execute_code_tool_call_blocks_codegraph_updates_without_permission() {
        let tool_result = execute_code_tool_call(
            Path::new("."),
            &json!({
                "id": "call-codegraph-sync",
                "function": {
                    "name": "codegraph_command",
                    "arguments": "{\"command\":\"sync\"}"
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

        TraceCtx::append_reasoning(
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
    fn chat_endpoint_accepts_base_or_full_endpoint() {
        assert_eq!(
            chat_completions_endpoint("https://api.example.com/v1"),
            "https://api.example.com/v1/chat/completions"
        );
        assert_eq!(
            chat_completions_endpoint("https://api.example.com/v1/chat/completions"),
            "https://api.example.com/v1/chat/completions"
        );
    }

    #[test]
    fn chat_endpoint_normalizes_ccswitch_codex_proxy_base() {
        assert_eq!(
            chat_completions_endpoint("http://127.0.0.1:15721/codex"),
            "http://127.0.0.1:15721/codex/v1/chat/completions"
        );
    }

    #[test]
    fn builds_openai_config_from_codex_toml_and_auth() {
        let config = build_ccswitch_openai_config(
            "test".to_string(),
            None,
            r#"
model_provider = "custom"
model = "gpt-5.5"

[model_providers.custom]
name = "Relay"
base_url = "https://relay.example.com/v1"
wire_api = "chat"
"#,
            Some(&json!({ "OPENAI_API_KEY": "sk-test" })),
            None,
            None,
        )
        .unwrap();

        assert_eq!(config.provider_name.as_deref(), Some("Relay"));
        assert_eq!(config.base_url, "https://relay.example.com/v1");
        assert_eq!(config.api_key, "sk-test");
        assert_eq!(config.model.as_deref(), Some("gpt-5.5"));
        assert_eq!(config.wire_api.as_deref(), Some("chat"));
        assert!(config.warning.is_none());
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

    #[test]
    fn detects_truncated_finish_reasons() {
        assert!(finish_reason_indicates_truncated_output(Some("length")));
        assert!(finish_reason_indicates_truncated_output(Some("MAX_TOKENS")));
        assert!(!finish_reason_indicates_truncated_output(Some("stop")));
        assert!(!finish_reason_indicates_truncated_output(None));
    }

    #[test]
    fn detects_incomplete_responses_output() {
        assert!(responses_output_is_incomplete(&json!({
            "status": "incomplete"
        })));
        assert!(responses_output_is_incomplete(&json!({
            "incomplete_details": { "reason": "max_output_tokens" }
        })));
        assert!(!responses_output_is_incomplete(&json!({
            "status": "completed"
        })));
    }

    #[test]
    fn combines_continued_output_segments() {
        let mut accumulated = String::new();
        append_continued_output(&mut accumulated, "Part one");
        append_continued_output(&mut accumulated, "Part two");

        assert_eq!(combined_output_text(&accumulated, "Part three"), "Part one\n\nPart two\n\nPart three");
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(ChatCancellationState::default())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            load_app_cache,
            save_app_cache,
            copy_avatar_to_cache,
            load_ccswitch_openai_config,
            chat_completion,
            cancel_chat_completion,
            finish_chat_completion,
            tools::codegraph::inspect_code_workspace,
            tools::patch::apply_patch_proposal
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
