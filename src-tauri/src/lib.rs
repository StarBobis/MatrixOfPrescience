use base64::{engine::general_purpose, Engine as _};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    fs,
    io::Write,
    path::{Component, Path, PathBuf},
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};
use tauri::{AppHandle, Manager};

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
    messages: Vec<ChatMessage>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ChatCompletionResponse {
    content: String,
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
        .map_err(|error| format!("Failed to decode cached avatar: {}", error))?;
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
        .map_err(|error| format!("Failed to copy avatar into cache: {}", error))?;

    Ok(AvatarCacheResponse {
        path: target.to_string_lossy().to_string(),
    })
}

#[tauri::command]
async fn chat_completion(request: ChatCompletionRequest) -> Result<ChatCompletionResponse, String> {
    if request.api_key.trim().is_empty() {
        return Err(format!("{} API Key 未配置", request.provider_name));
    }

    if request.model.trim().is_empty() {
        return Err("模型名称不能为空".to_string());
    }

    let mut messages = Vec::new();
    if let Some(system_prompt) = request.system_prompt.as_deref() {
        let trimmed = system_prompt.trim();
        if !trimmed.is_empty() {
            messages.push(json!({
                "role": "system",
                "content": trimmed,
            }));
        }
    }

    for message in request.messages {
        messages.push(json!({
            "role": message.role,
            "content": message.content,
        }));
    }

    let endpoint = format!(
        "{}/chat/completions",
        request.base_url.trim().trim_end_matches('/')
    );

    let mut payload = json!({
        "model": request.model,
        "messages": messages,
        "temperature": request.temperature.unwrap_or(0.7),
    });

    if let Some(reasoning_effort) = request.reasoning_effort.as_deref() {
        let trimmed = reasoning_effort.trim();
        if !trimmed.is_empty() && trimmed != "off" {
            payload["reasoning_effort"] = json!(trimmed);
        }
    }

    let client = reqwest::Client::new();
    let response = client
        .post(endpoint)
        .bearer_auth(request.api_key.trim())
        .json(&payload)
        .send()
        .await
        .map_err(|error| format!("请求 {} 失败：{}", request.provider_name, error))?;

    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|error| format!("读取 {} 响应失败：{}", request.provider_name, error))?;

    if !status.is_success() {
        return Err(format!(
            "{} 返回 HTTP {}：{}",
            request.provider_name, status, body
        ));
    }

    let parsed: Value = serde_json::from_str(&body)
        .map_err(|error| format!("解析 {} 响应失败：{}", request.provider_name, error))?;

    let content = parsed
        .get("choices")
        .and_then(|choices| choices.get(0))
        .and_then(|choice| choice.get("message"))
        .and_then(|message| message.get("content"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();

    if content.is_empty() {
        return Err(format!("{} 未返回可显示的消息内容", request.provider_name));
    }

    Ok(ChatCompletionResponse { content })
}

fn validate_workspace(workspace_path: &str) -> Result<PathBuf, String> {
    let trimmed = workspace_path.trim();

    if trimmed.is_empty() {
        return Err("工作文件夹不能为空".to_string());
    }

    let workspace = fs::canonicalize(trimmed)
        .map_err(|error| format!("工作文件夹不存在或不可访问：{}", error))?;

    if !workspace.is_dir() {
        return Err("工作文件夹必须是目录".to_string());
    }

    Ok(workspace)
}

fn has_codegraph_index(workspace: &Path) -> bool {
    workspace
        .ancestors()
        .any(|path| path.join(".codegraph").is_dir())
}

fn truncate_text(text: String, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text;
    }

    let mut truncated: String = text.chars().take(max_chars).collect();
    truncated.push_str("\n\n[内容已截断]");
    truncated
}

fn run_codegraph_explore(workspace: &Path, query: &str) -> Result<String, String> {
    if !has_codegraph_index(workspace) {
        return Err("当前工作文件夹未发现 .codegraph 索引".to_string());
    }

    let output = Command::new("codegraph")
        .current_dir(workspace)
        .arg("explore")
        .arg(query)
        .output()
        .map_err(|error| format!("启动 CodeGraph 失败：{}", error))?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    if !output.status.success() || stdout.trim().is_empty() {
        return Err(if stderr.trim().is_empty() {
            "CodeGraph 未返回可用结果".to_string()
        } else {
            format!("CodeGraph 查询失败：{}", stderr.trim())
        });
    }

    Ok(truncate_text(stdout, 18_000))
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
        .map_err(|error| format!("本地文件列表读取失败：{}", error))?;

    if !output.status.success() {
        return Err("本地文件列表读取失败".to_string());
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
        "CodeGraph 不可用，已降级为本地命令读取。".to_string(),
        "文件列表：".to_string(),
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
        return Err("代码阅读问题不能为空".to_string());
    }

    match run_codegraph_explore(&workspace, query) {
        Ok(content) => Ok(InspectCodeWorkspaceResponse {
            tool: "CodeGraph".to_string(),
            content,
        }),
        Err(codegraph_error) => {
            let fallback = read_local_code_context(&workspace, query)?;
            Ok(InspectCodeWorkspaceResponse {
                tool: format!("LocalCommands（CodeGraph 失败：{}）", codegraph_error),
                content: fallback,
            })
        }
    }
}

fn validate_relative_file(file: &str) -> Result<String, String> {
    let normalized = file.trim().replace('\\', "/");

    if normalized.is_empty() {
        return Err("补丁包含空文件路径".to_string());
    }

    let path = Path::new(&normalized);

    if path.is_absolute() {
        return Err(format!("拒绝绝对路径：{}", normalized));
    }

    for component in path.components() {
        match component {
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(format!("拒绝越界路径：{}", normalized));
            }
            Component::Normal(part) => {
                let text = part.to_string_lossy().to_ascii_lowercase();
                if matches!(
                    text.as_str(),
                    ".ssh" | ".aws" | ".config" | "node_modules" | "dist" | "target"
                ) || text.starts_with(".env")
                {
                    return Err(format!("拒绝敏感或生成目录路径：{}", normalized));
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
        return Err("无法识别补丁目标文件".to_string());
    }

    Ok(validated)
}

fn write_temp_patch(patch_text: &str) -> Result<PathBuf, String> {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| format!("生成临时补丁名失败：{}", error))?
        .as_millis();
    let path = std::env::temp_dir().join(format!("matrixofprescience-{}.patch", stamp));
    let mut file =
        fs::File::create(&path).map_err(|error| format!("创建临时补丁失败：{}", error))?;

    file.write_all(patch_text.as_bytes())
        .map_err(|error| format!("写入临时补丁失败：{}", error))?;

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
        .map_err(|error| format!("执行 git apply 失败：{}", error))?;
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    if !output.status.success() {
        return Err(if stderr.trim().is_empty() {
            format!("git apply 失败：{}", stdout)
        } else {
            format!("git apply 失败：{}", stderr)
        });
    }

    Ok((stdout, stderr))
}

#[tauri::command]
async fn apply_patch_proposal(request: ApplyPatchRequest) -> Result<ApplyPatchResponse, String> {
    let workspace = validate_workspace(&request.workspace_path)?;
    let patch_text = request.patch_text.trim();

    if patch_text.is_empty() {
        return Err("补丁内容不能为空".to_string());
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
