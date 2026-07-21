use std::fs;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags};
use serde::Serialize;
use serde_json::Value;

use crate::*;

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CcSwitchOpenAiConfig {
    pub(crate) source: String,
    pub(crate) provider_name: Option<String>,
    pub(crate) base_url: String,
    pub(crate) api_key: String,
    pub(crate) model: Option<String>,
    pub(crate) wire_api: Option<String>,
    pub(crate) warning: Option<String>,
}
pub(crate) fn app_data_dir(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_data_dir()
        .map_err(|error| format!("Failed to resolve app data directory: {}", error))
}

pub(crate) fn read_json_file(path: &Path) -> Result<Option<Value>, String> {
    if !path.exists() {
        return Ok(None);
    }

    let content = fs::read_to_string(path)
        .map_err(|error| format!("Failed to read {:?}: {}", path, error))?;

    serde_json::from_str(&content)
        .map(Some)
        .map_err(|error| format!("Failed to parse {:?}: {}", path, error))
}

pub(crate) fn write_json_file(path: &Path, value: &Value) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("Failed to create {:?}: {}", parent, error))?;
    }

    let content = serde_json::to_string_pretty(value)
        .map_err(|error| format!("Failed to serialize {:?}: {}", path, error))?;

    fs::write(path, content).map_err(|error| format!("Failed to write {:?}: {}", path, error))
}

pub(crate) fn user_home_dir() -> Option<PathBuf> {
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

pub(crate) fn codex_config_dirs() -> Vec<PathBuf> {
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

pub(crate) fn ccswitch_config_dir() -> Result<PathBuf, String> {
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

pub(crate) fn active_codex_provider<'a>(doc: &'a toml::Value) -> (Option<String>, Option<&'a toml::Value>) {
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

pub(crate) fn codex_toml_field(doc: Option<&toml::Value>, key: &str) -> Option<String> {
    let Some(doc) = doc else {
        return None;
    };
    let (_, active_provider_config) = active_codex_provider(doc);

    StrUtils::toml_str(active_provider_config, key).or_else(|| StrUtils::toml_str(Some(doc), key))
}

pub(crate) fn codex_auth_api_key(auth: Option<&Value>) -> Option<String> {
    StrUtils::json_str(auth, "OPENAI_API_KEY")
}

pub(crate) fn is_local_ccswitch_proxy_url(base_url: &str) -> bool {
    let lower = base_url.trim().trim_end_matches('/').to_ascii_lowercase();
    let is_local = lower.starts_with("http://127.0.0.1:")
        || lower.starts_with("http://localhost:")
        || lower.starts_with("http://[::1]:");

    is_local
        && (lower.contains("/codex")
            || lower.starts_with("http://127.0.0.1:15721")
            || lower.starts_with("http://localhost:15721"))
}

pub(crate) fn normalize_imported_base_url(base_url: &str) -> String {
    let trimmed = base_url.trim().trim_end_matches('/').to_string();
    let lower = trimmed.to_ascii_lowercase();

    if is_local_ccswitch_proxy_url(&trimmed) && lower.ends_with("/codex") {
        return format!("{}/v1", trimmed);
    }

    trimmed
}

pub(crate) fn imported_config_warning(
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

pub(crate) fn build_ccswitch_openai_config(
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

pub(crate) fn load_ccswitch_openai_config_from_codex_live() -> Result<CcSwitchOpenAiConfig, String> {
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

pub(crate) fn load_ccswitch_openai_config_from_database() -> Result<CcSwitchOpenAiConfig, String> {
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
pub fn load_ccswitch_openai_config() -> Result<CcSwitchOpenAiConfig, String> {
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

