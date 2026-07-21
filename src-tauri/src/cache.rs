use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::{AppHandle, Manager};

use crate::*;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SaveAppCacheRequest {
    pub(crate) cache_directory: Option<String>,
    pub(crate) settings: Value,
    pub(crate) member_library: Value,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AppCacheState {
    pub(crate) default_cache_directory: String,
    pub(crate) cache_directory: String,
    pub(crate) settings: Option<Value>,
    pub(crate) member_library: Option<Value>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AvatarCacheResponse {
    pub(crate) path: String,
}

pub(crate) fn read_cache_directory(app: &AppHandle) -> Result<PathBuf, String> {
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

pub(crate) fn write_cache_directory(app: &AppHandle, cache_directory: &Path) -> Result<(), String> {
    let default_dir = app_data_dir(app)?;
    fs::create_dir_all(&default_dir)
        .map_err(|error| format!("Failed to create {:?}: {}", default_dir, error))?;
    write_json_file(
        &default_dir.join(CACHE_LOCATION_FILE),
        &json!({ "cacheDirectory": cache_directory.to_string_lossy() }),
    )
}

pub(crate) fn ensure_cache_tree(cache_directory: &Path) -> Result<(), String> {
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
pub async fn load_app_cache(app: AppHandle) -> Result<AppCacheState, String> {
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
pub async fn save_app_cache(app: AppHandle, mut request: SaveAppCacheRequest) -> Result<(), String> {
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

pub(crate) fn avatar_extension_from_mime(mime_type: &str) -> &str {
    match mime_type {
        "image/jpeg" => "jpg",
        "image/svg+xml" => "svg",
        "image/webp" => "webp",
        "image/gif" => "gif",
        "image/bmp" => "bmp",
        _ => "png",
    }
}

pub(crate) fn write_avatar_data_url(value: &str, cache_directory: &Path) -> Result<Option<String>, String> {
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

pub(crate) fn materialize_avatar_data_urls(value: &mut Value, cache_directory: &Path) -> Result<(), String> {
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

pub(crate) fn is_supported_avatar(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|value| value.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some("png" | "jpg" | "jpeg" | "webp" | "gif" | "bmp" | "svg")
    )
}

#[tauri::command]
pub async fn copy_avatar_to_cache(
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
