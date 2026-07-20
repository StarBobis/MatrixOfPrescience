use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const SPILL_DIR_RELATIVE: &str = ".matrix-cache/tool-results";
const MAX_SPILL_FILES: usize = 20;

/// Returns `text` unchanged when it fits. Otherwise writes the full text to a
/// workspace cache file and returns a preview plus a pointer, so the model can
/// page through the complete result with read_file instead of losing the tail
/// to a hard "[Content truncated]" cut.
pub(crate) fn spill_tool_output(
    workspace: &Path,
    label: &str,
    text: String,
    max_chars: usize,
) -> String {
    let total_chars = text.chars().count();

    if total_chars <= max_chars {
        return text;
    }

    let preview: String = text.chars().take(max_chars).collect();

    match write_spill_file(workspace, label, &text) {
        Ok(path) => {
            let display = path
                .strip_prefix(workspace)
                .map(|relative| relative.to_string_lossy().replace('\\', "/"))
                .unwrap_or_else(|_| path.display().to_string());
            format!(
                "{preview}\n\n[Showing first {max_chars} of {total_chars} chars. Full output saved to {display} — use read_file with startLine/maxLines to read the rest.]"
            )
        }
        Err(error) => format!(
            "{preview}\n\n[Showing first {max_chars} of {total_chars} chars; the remaining {} chars could not be saved: {error}]",
            total_chars.saturating_sub(max_chars)
        ),
    }
}

fn write_spill_file(workspace: &Path, label: &str, text: &str) -> Result<PathBuf, String> {
    let dir = workspace.join(SPILL_DIR_RELATIVE);
    fs::create_dir_all(&dir)
        .map_err(|error| format!("failed to create {}: {error}", dir.display()))?;
    prune_spill_dir(&dir);

    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0);
    let path = dir.join(format!("{label}-{stamp}-{}.txt", std::process::id()));
    fs::write(&path, text)
        .map_err(|error| format!("failed to write {}: {error}", path.display()))?;
    Ok(path)
}

fn prune_spill_dir(dir: &Path) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };

    let mut files: Vec<(SystemTime, PathBuf)> = entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            if path.extension().and_then(|value| value.to_str()) != Some("txt") {
                return None;
            }
            let modified = entry
                .metadata()
                .and_then(|metadata| metadata.modified())
                .unwrap_or(UNIX_EPOCH);
            Some((modified, path))
        })
        .collect();

    if files.len() < MAX_SPILL_FILES {
        return;
    }

    files.sort_by_key(|(modified, _)| *modified);
    for (_, path) in files.iter().take(files.len() + 1 - MAX_SPILL_FILES) {
        let _ = fs::remove_file(path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_workspace(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "mop-spill-test-{name}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|duration| duration.as_nanos())
                .unwrap_or(0)
        ));
        fs::create_dir_all(&dir).expect("temp workspace should be created");
        dir
    }

    #[test]
    fn small_output_is_returned_unchanged_without_writing_a_file() {
        let workspace = temp_workspace("small");
        let output = spill_tool_output(&workspace, "web-search", "short result".to_string(), 100);

        assert_eq!(output, "short result");
        assert!(!workspace.join(SPILL_DIR_RELATIVE).exists());
        let _ = fs::remove_dir_all(&workspace);
    }

    #[test]
    fn large_output_is_spilled_to_a_readable_file_with_a_pointer() {
        let workspace = temp_workspace("large");
        let text = "x".repeat(500);
        let output = spill_tool_output(&workspace, "web-search", text.clone(), 100);

        assert!(output.starts_with(&"x".repeat(100)));
        assert!(output.contains("Showing first 100 of 500 chars"));
        assert!(output.contains(".matrix-cache/tool-results/web-search-"));
        assert!(output.contains("read_file"));

        let spill_dir = workspace.join(SPILL_DIR_RELATIVE);
        let files: Vec<_> = fs::read_dir(&spill_dir)
            .expect("spill dir should exist")
            .flatten()
            .collect();
        assert_eq!(files.len(), 1);
        let saved = fs::read_to_string(files[0].path()).expect("spill file should be readable");
        assert_eq!(saved, text);
        let _ = fs::remove_dir_all(&workspace);
    }

    #[test]
    fn unwritable_spill_dir_falls_back_to_an_omitted_count_notice() {
        let workspace = temp_workspace("readonly");
        // Make the spill path a regular file so create_dir_all fails.
        fs::write(workspace.join(".matrix-cache"), "block").expect("blocker file should be written");

        let text = "y".repeat(300);
        let output = spill_tool_output(&workspace, "web-search", text, 100);

        assert!(output.starts_with(&"y".repeat(100)));
        assert!(output.contains("Showing first 100 of 300 chars"));
        assert!(output.contains("200 chars could not be saved"));
        let _ = fs::remove_dir_all(&workspace);
    }
}
