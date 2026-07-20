use std::{
    collections::{hash_map::DefaultHasher, HashMap},
    fs,
    hash::{Hash, Hasher},
    path::Path,
    sync::{Mutex, OnceLock},
};

/// Session-level read-before-edit guard: a model must read a file with
/// read_file before write_file/apply_patch may change it, and must re-read
/// it whenever the on-disk content changed since the last recorded read.
static READ_LOG: OnceLock<Mutex<HashMap<String, u64>>> = OnceLock::new();

fn read_log() -> &'static Mutex<HashMap<String, u64>> {
    READ_LOG.get_or_init(|| Mutex::new(HashMap::new()))
}

fn hash_content(content: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    content.hash(&mut hasher);
    hasher.finish()
}

fn key(path: &Path) -> String {
    path.to_string_lossy().to_string()
}

pub(crate) fn record_read(path: &Path, content: &str) {
    if let Ok(mut log) = read_log().lock() {
        log.insert(key(path), hash_content(content));
    }
}

pub(crate) fn record_write(path: &Path, content: &str) {
    record_read(path, content);
}

/// Ok(()) when the file may be edited: it does not exist yet, or it was
/// read before and is unchanged since. Err explains which read is missing.
pub(crate) fn check_before_edit(path: &Path, display: &str) -> Result<(), String> {
    if !path.exists() {
        return Ok(());
    }

    // Non-text targets stay outside the guard: they cannot be read via read_file.
    let Ok(current) = fs::read_to_string(path) else {
        return Ok(());
    };

    let recorded = read_log()
        .lock()
        .ok()
        .and_then(|log| log.get(&key(path)).copied());

    match recorded {
        None => Err(format!(
            "read_before_edit_required: read {} with read_file before editing it.",
            display
        )),
        Some(hash) if hash != hash_content(&current) => Err(format!(
            "read_before_edit_required: {} changed on disk since your last read; read it again with read_file before editing.",
            display
        )),
        Some(_) => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_file(tag: &str, content: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "matrixofprescience-read-guard-{}-{}",
            std::process::id(),
            tag
        ));
        fs::write(&path, content).unwrap();
        path
    }

    #[test]
    fn missing_file_needs_no_read() {
        let path = std::env::temp_dir().join(format!(
            "matrixofprescience-read-guard-{}-absent",
            std::process::id()
        ));
        let _ = fs::remove_file(&path);

        assert!(check_before_edit(&path, "absent.txt").is_ok());
    }

    #[test]
    fn existing_file_requires_fresh_read_before_edit() {
        let path = temp_file("flow.txt", "one");

        // Never read: blocked.
        assert!(check_before_edit(&path, "flow.txt").is_err());

        // Read once: allowed.
        let content = fs::read_to_string(&path).unwrap();
        record_read(&path, &content);
        assert!(check_before_edit(&path, "flow.txt").is_ok());

        // External change after the read: blocked as stale.
        fs::write(&path, "two").unwrap();
        let stale_error = check_before_edit(&path, "flow.txt").unwrap_err();
        assert!(stale_error.contains("changed on disk"));

        // Successful write updates the record: allowed again.
        record_write(&path, "two");
        assert!(check_before_edit(&path, "flow.txt").is_ok());

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn edit_without_read_reports_read_before_edit_required() {
        let path = temp_file("unread.txt", "seed");

        let error = check_before_edit(&path, "unread.txt").unwrap_err();
        assert!(error.contains("read_before_edit_required"));

        let _ = fs::remove_file(&path);
    }
}
