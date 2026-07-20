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

/// Normalizes a path so every tool resolves to the same read-log key:
/// strips the Windows verbatim prefix (`\\?\`) produced by fs::canonicalize,
/// unifies separators, and folds case on Windows. Without this, paths from
/// `resolve_workspace_relative_path` (canonicalized) and
/// `resolve_workspace_target_path` (plain join) never map to the same entry
/// and write_file stays blocked by read_before_edit_required forever.
fn key(path: &Path) -> String {
    // Canonicalize through the OS first: this bridges every spelling
    // difference at once (verbatim prefix, 8.3 short vs long names, casing,
    // separators, symlinks). Non-existent paths fall back to the raw path.
    let resolved = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let mut text = resolved.to_string_lossy().replace('\\', "/");

    if let Some(stripped) = text.strip_prefix("//?/") {
        text = stripped.to_string();
    }

    if cfg!(windows) {
        text = text.to_ascii_lowercase();
    }

    text
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
            "read_before_edit_required: read {} with the read_file tool immediately before editing it (search_files/codegraph results do not count as a read).",
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

    #[cfg(windows)]
    #[test]
    fn key_normalizes_verbatim_prefix_slashes_and_case() {
        let verbatim = Path::new(r"\\?\D:\Dev\Proj\src\Main.rs");
        let plain = Path::new("d:/dev/proj/src/main.rs");
        assert_eq!(key(verbatim), key(plain));
    }

    #[cfg(windows)]
    #[test]
    fn read_recorded_under_canonical_path_satisfies_plain_write_path() {
        // Reproduces the write_file deadlock: read_file records under the
        // canonicalized (\\?\-prefixed) path while write_file's resolver
        // checks the plain joined path.
        let path = temp_file("verbatim.txt", "seed");
        let canonical = fs::canonicalize(&path).unwrap();
        let content = fs::read_to_string(&path).unwrap();
        record_read(&canonical, &content);

        let plain = std::path::PathBuf::from(path.display().to_string().to_uppercase());
        assert!(check_before_edit(&plain, "VERBATIM.TXT").is_ok());

        let _ = fs::remove_file(&path);
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
