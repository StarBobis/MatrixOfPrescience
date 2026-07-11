use serde_json::Value;
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

use crate::tools::execution::{
    resolve_workspace_relative_path, tool_arg_bool, validate_relative_file, validate_workspace,
    ApplyPatchRequest, ApplyPatchResponse,
};
use crate::utils::string_utils::StrUtils;

fn write_temp_patch(patch_text: &str) -> Result<PathBuf, String> {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| format!("Failed to create temporary patch name: {}", error))?
        .as_nanos();
    let process_id = std::process::id();

    for attempt in 0..100_u8 {
        let path = std::env::temp_dir().join(format!(
            "matrixofprescience-{}-{}-{}.patch",
            process_id, stamp, attempt
        ));
        let file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path);

        match file {
            Ok(mut file) => {
                file.write_all(patch_text.as_bytes())
                    .map_err(|error| format!("Failed to write temporary patch: {}", error))?;
                return Ok(path);
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(format!("Failed to create temporary patch: {}", error)),
        }
    }

    Err("Failed to create a unique temporary patch after 100 attempts.".to_string())
}

fn parse_hunk_header(line: &str) -> Result<(&str, &str, &str), String> {
    let rest = line
        .strip_prefix("@@ -")
        .ok_or_else(|| format!("Invalid unified diff hunk header: {}", line))?;
    let (old_range, rest) = rest
        .split_once(" +")
        .ok_or_else(|| format!("Invalid unified diff hunk header: {}", line))?;
    let (new_range, suffix) = rest
        .split_once(" @@")
        .ok_or_else(|| format!("Invalid unified diff hunk header: {}", line))?;
    let old_start = old_range.split(',').next().unwrap_or_default();
    let new_start = new_range.split(',').next().unwrap_or_default();

    if old_start.parse::<u64>().is_err() || new_start.parse::<u64>().is_err() {
        return Err(format!("Invalid unified diff hunk header: {}", line));
    }

    Ok((old_start, new_start, suffix))
}

fn is_hunk_boundary(line: &str) -> bool {
    line.starts_with("@@ -") || line.starts_with("diff --git ")
}

fn normalize_patch_metadata_line(line: &str) -> Option<String> {
    for dash in ["\u{2013}", "\u{2014}", "\u{2015}", "\u{2212}"] {
        if let Some(path) = line.strip_prefix(&format!("{} ", dash)) {
            if path.starts_with("a/") || path == "/dev/null" {
                return Some(format!("--- {}", path));
            }
        }
    }

    None
}

fn normalize_relaxed_unified_diff(patch_text: &str) -> Result<String, String> {
    let normalized_line_endings = patch_text.replace("\r\n", "\n").replace('\r', "\n");
    let lines: Vec<&str> = normalized_line_endings.lines().collect();
    let mut output = String::with_capacity(normalized_line_endings.len() + lines.len());
    let mut index = 0;
    let mut hunk_count = 0;

    while index < lines.len() {
        let line = lines[index];
        if !line.starts_with("@@ -") {
            if let Some(normalized_line) = normalize_patch_metadata_line(line) {
                output.push_str(&normalized_line);
            } else {
                output.push_str(line);
            }
            output.push('\n');
            index += 1;
            continue;
        }

        let (old_start, new_start, suffix) = parse_hunk_header(line)?;
        let body_start = index + 1;
        let mut body_end = body_start;
        let mut old_count = 0_u64;
        let mut new_count = 0_u64;

        while body_end < lines.len() && !is_hunk_boundary(lines[body_end]) {
            match lines[body_end].as_bytes().first().copied() {
                Some(b' ') => {
                    old_count += 1;
                    new_count += 1;
                }
                Some(b'-') => old_count += 1,
                Some(b'+') => new_count += 1,
                Some(b'\\') => {}
                _ => {
                    old_count += 1;
                    new_count += 1;
                }
            }
            body_end += 1;
        }

        if body_start == body_end {
            return Err(format!("Unified diff hunk has no body: {}", line));
        }

        output.push_str(&format!(
            "@@ -{},{} +{},{} @@{}\n",
            old_start, old_count, new_start, new_count, suffix
        ));
        for body_line in &lines[body_start..body_end] {
            match body_line.as_bytes().first().copied() {
                Some(b' ') | Some(b'-') | Some(b'+') | Some(b'\\') => {}
                _ => output.push(' '),
            }
            output.push_str(body_line);
            output.push('\n');
        }

        hunk_count += 1;
        index = body_end;
    }

    if hunk_count == 0 {
        return Err("No unified diff hunks were found to normalize.".to_string());
    }

    Ok(output)
}

fn git_apply_error_allows_normalization(error: &str) -> bool {
    let lower = error.to_ascii_lowercase();
    [
        "corrupt patch",
        "patch fragment without header",
        "patch with only garbage",
        "unrecognized input",
    ]
    .iter()
    .any(|message| lower.contains(message))
}

fn prepare_checked_patch(workspace: &Path, patch_text: &str) -> Result<(PathBuf, bool), String> {
    let patch_file = write_temp_patch(patch_text)?;
    let original_error = match run_git_apply(workspace, &patch_file, true) {
        Ok(_) => return Ok((patch_file, false)),
        Err(error) => error,
    };

    if !git_apply_error_allows_normalization(&original_error) {
        let _ = fs::remove_file(&patch_file);
        return Err(original_error);
    }

    let normalized = match normalize_relaxed_unified_diff(patch_text) {
        Ok(normalized) if normalized != patch_text => normalized,
        Ok(_) => {
            let _ = fs::remove_file(&patch_file);
            return Err(format!(
                "{}\nThe patch is structurally corrupt and no safe normalization was available.",
                original_error
            ));
        }
        Err(normalize_error) => {
            let _ = fs::remove_file(&patch_file);
            return Err(format!(
                "{}\nAutomatic patch normalization failed: {}",
                original_error, normalize_error
            ));
        }
    };

    if let Err(error) = fs::write(&patch_file, normalized.as_bytes()) {
        let _ = fs::remove_file(&patch_file);
        return Err(format!(
            "Failed to write normalized temporary patch: {}",
            error
        ));
    }

    match run_git_apply(workspace, &patch_file, true) {
        Ok(_) => Ok((patch_file, true)),
        Err(normalized_error) => {
            let _ = fs::remove_file(&patch_file);
            Err(format!(
                "{}\nThe patch format was normalized, but git still rejected it: {}",
                original_error, normalized_error
            ))
        }
    }
}

fn normalize_match_line(line: &str) -> String {
    line.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn find_line_fragment(lines: &[String], fragment: &[String], start_hint: usize) -> Option<usize> {
    if fragment.is_empty() {
        return Some(start_hint.min(lines.len()));
    }

    if fragment.len() > lines.len() {
        return None;
    }

    let max_start = lines.len() - fragment.len();
    let start_hint = start_hint.min(max_start);
    let mut candidates = Vec::with_capacity(max_start + 1);
    candidates.push(start_hint);

    for distance in 1..=max_start.max(start_hint) {
        if let Some(before) = start_hint.checked_sub(distance) {
            candidates.push(before);
        }
        let after = start_hint + distance;
        if after <= max_start {
            candidates.push(after);
        }
    }
    candidates.sort_unstable();
    candidates.dedup();

    for start in &candidates {
        if lines[*start..*start + fragment.len()] == *fragment {
            return Some(*start);
        }
    }

    let normalized_fragment = fragment
        .iter()
        .map(|line| normalize_match_line(line))
        .collect::<Vec<_>>();
    candidates.into_iter().find(|start| {
        lines[*start..*start + fragment.len()]
            .iter()
            .map(|line| normalize_match_line(line))
            .eq(normalized_fragment.iter().cloned())
    })
}

fn apply_patch_by_line_fragments(
    workspace: &Path,
    patch_file: &Path,
    check_only: bool,
) -> Result<(String, String), String> {
    let patch_text = fs::read_to_string(patch_file)
        .map_err(|error| format!("Failed to read patch for fallback apply: {}", error))?;
    let normalized_patch = normalize_relaxed_unified_diff(&patch_text)?;
    let patch_lines = normalized_patch.lines().collect::<Vec<_>>();
    let mut index = 0;
    let mut pending_writes: Vec<(PathBuf, String)> = Vec::new();

    while index < patch_lines.len() {
        if !patch_lines[index].starts_with("diff --git ") {
            index += 1;
            continue;
        }

        let mut target_file = None;
        index += 1;
        while index < patch_lines.len() && !patch_lines[index].starts_with("@@ -") {
            if let Some(path) = patch_lines[index].strip_prefix("+++ b/") {
                target_file = Some(validate_relative_file(path)?);
            }
            index += 1;
        }

        let Some(target_file) = target_file else {
            return Err("Fallback patch apply could not detect a target file.".to_string());
        };
        let target_path = resolve_workspace_relative_path(workspace, &target_file)?;
        let existing = fs::read_to_string(&target_path)
            .map_err(|error| format!("Fallback patch apply failed to read {}: {}", target_file, error))?;
        let normalized_existing = existing.replace("\r\n", "\n").replace('\r', "\n");
        let had_trailing_newline = normalized_existing.ends_with('\n');
        let mut file_lines = normalized_existing
            .split('\n')
            .map(str::to_string)
            .collect::<Vec<_>>();
        if had_trailing_newline {
            file_lines.pop();
        }

        while index < patch_lines.len() && patch_lines[index].starts_with("@@ -") {
            let (old_start, _, _) = parse_hunk_header(patch_lines[index])?;
            let start_hint = old_start
                .parse::<usize>()
                .ok()
                .and_then(|value| value.checked_sub(1))
                .unwrap_or(0);
            index += 1;

            let mut old_fragment = Vec::new();
            let mut new_fragment = Vec::new();
            while index < patch_lines.len() && !is_hunk_boundary(patch_lines[index]) {
                let line = patch_lines[index];
                match line.as_bytes().first().copied() {
                    Some(b' ') => {
                        old_fragment.push(line[1..].to_string());
                        new_fragment.push(line[1..].to_string());
                    }
                    Some(b'-') => old_fragment.push(line[1..].to_string()),
                    Some(b'+') => new_fragment.push(line[1..].to_string()),
                    Some(b'\\') => {}
                    _ => {
                        old_fragment.push(line.to_string());
                        new_fragment.push(line.to_string());
                    }
                }
                index += 1;
            }

            let Some(fragment_start) = find_line_fragment(&file_lines, &old_fragment, start_hint)
            else {
                return Err(format!(
                    "Fallback patch apply could not find the expected text in {}.",
                    target_file
                ));
            };
            file_lines.splice(
                fragment_start..fragment_start + old_fragment.len(),
                new_fragment.into_iter(),
            );
        }

        let mut next_content = file_lines.join("\n");
        if had_trailing_newline {
            next_content.push('\n');
        }
        pending_writes.push((target_path, next_content));
    }

    if pending_writes.is_empty() {
        return Err("Fallback patch apply found no text hunks.".to_string());
    }

    if !check_only {
        for (path, content) in pending_writes {
            fs::write(path, content)
                .map_err(|error| format!("Fallback patch apply failed to write file: {}", error))?;
        }
    }

    Ok((
        "Patch applied with text-fragment fallback.".to_string(),
        String::new(),
    ))
}

fn run_git_apply(
    workspace: &Path,
    patch_file: &Path,
    check_only: bool,
) -> Result<(String, String), String> {
    let attempts: &[&[&str]] = &[
        &[],
        &["--recount"],
        &["--ignore-space-change", "--ignore-whitespace"],
        &["--unidiff-zero"],
    ];
    let mut failures = Vec::new();

    for extra_args in attempts {
        let mut command = Command::new("git");
        command
            .current_dir(workspace)
            .arg("apply")
            .arg("--whitespace=nowarn")
            .args(*extra_args);

        if check_only {
            command.arg("--check");
        }

        let output = command
            .arg(patch_file)
            .output()
            .map_err(|error| format!("Failed to run git apply: {}", error))?;
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();

        if output.status.success() {
            return Ok((stdout, stderr));
        }

        let label = if extra_args.is_empty() {
            "default".to_string()
        } else {
            extra_args.join(" ")
        };
        let message = if stderr.trim().is_empty() {
            stdout
        } else {
            stderr
        };
        failures.push(format!("{label}: {}", message.trim()));
    }

    // If all git apply attempts failed, try the text-fragment fallback
    apply_patch_by_line_fragments(workspace, patch_file, check_only)
        .map_err(|fallback_error| {
            format!(
                "git apply failed:\n{}\ntext-fragment fallback failed: {}",
                failures.join("\n"),
                fallback_error
            )
        })
}

pub(crate) fn apply_patch_tool(workspace: &Path, arguments: &Value) -> Result<String, String> {
    let patch_text = arguments
        .get("patchText")
        .and_then(Value::as_str)
        .unwrap_or("");
    let check_only = tool_arg_bool(arguments, "checkOnly", false);

    if patch_text.trim().is_empty() {
        return Err("apply_patch requires patchText.".to_string());
    }

    let request = ApplyPatchRequest {
        workspace_path: workspace.to_string_lossy().to_string(),
        patch_text: patch_text.to_string(),
        files: Vec::new(),
    };
    let applied_files = collect_patch_files(&request)?;
    let (patch_file, normalized) = prepare_checked_patch(workspace, patch_text)?;

    if check_only {
        let _ = fs::remove_file(&patch_file);
        return Ok(format!(
            "Patch check passed for files:\n{}",
            applied_files.join("\n")
        ));
    }

    let apply_result = run_git_apply(workspace, &patch_file, false);
    let _ = fs::remove_file(&patch_file);
    let (stdout, stderr) = apply_result?;
    let output = [stdout.trim(), stderr.trim()]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("\n");

    Ok(format!(
        "Patch applied to files:\n{}\n{}{}",
        applied_files.join("\n"),
        if normalized {
            "Patch format was normalized before applying (missing context markers and/or incorrect hunk counts were repaired).\n"
        } else {
            ""
        },
        if output.is_empty() {
            "git apply produced no output.".to_string()
        } else {
            StrUtils::truncate_text(output, 8_000)
        }
    ))
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

#[tauri::command]
pub(crate) async fn apply_patch_proposal(
    request: ApplyPatchRequest,
) -> Result<ApplyPatchResponse, String> {
    let workspace = validate_workspace(&request.workspace_path)?;
    let patch_text = request.patch_text.as_str();

    if patch_text.trim().is_empty() {
        return Err("Patch content cannot be empty.".to_string());
    }

    let applied_files = collect_patch_files(&request)?;
    let (patch_file, _) = prepare_checked_patch(&workspace, patch_text)?;

    let result = (|| {
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
