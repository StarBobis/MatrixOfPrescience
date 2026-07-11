use std::path::PathBuf;
use tauri::Manager;

#[allow(dead_code)]
pub(crate) struct FileUtils;

impl FileUtils {
    pub(crate) fn app_data_dir(app: &tauri::AppHandle) -> Result<PathBuf, String> {
        app.path()
            .app_data_dir()
            .map_err(|error| format!("Failed to resolve app data directory: {}", error))
    }

    pub(crate) fn read_json(path: &std::path::Path) -> Result<Option<serde_json::Value>, String> {
        if !path.exists() {
            return Ok(None);
        }

        let content = std::fs::read_to_string(path)
            .map_err(|error| format!("Failed to read {:?}: {}", path, error))?;

        serde_json::from_str(&content)
            .map(Some)
            .map_err(|error| format!("Failed to parse {:?}: {}", path, error))
    }

    pub(crate) fn write_json(
        path: &std::path::Path,
        value: &serde_json::Value,
    ) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| format!("Failed to create {:?}: {}", parent, error))?;
        }

        let content = serde_json::to_string_pretty(value)
            .map_err(|error| format!("Failed to serialize {:?}: {}", path, error))?;

        std::fs::write(path, content)
            .map_err(|error| format!("Failed to write {:?}: {}", path, error))
    }

    pub(crate) fn user_home() -> Option<PathBuf> {
        std::env::var("USERPROFILE")
            .ok()
            .and_then(|value| {
                let trimmed = value.trim();
                (!trimmed.is_empty()).then(|| trimmed.to_string())
            })
            .map(PathBuf::from)
            .or_else(|| {
                let drive = std::env::var("HOMEDRIVE").ok()?;
                let path = std::env::var("HOMEPATH").ok()?;
                let combined = format!("{}{}", drive, path);
                let trimmed = combined.trim();
                (!trimmed.is_empty()).then(|| PathBuf::from(trimmed))
            })
            .or_else(|| {
                std::env::var("HOME")
                    .ok()
                    .and_then(|value| {
                        let trimmed = value.trim();
                        (!trimmed.is_empty()).then(|| trimmed.to_string())
                    })
                    .map(PathBuf::from)
            })
    }
}
