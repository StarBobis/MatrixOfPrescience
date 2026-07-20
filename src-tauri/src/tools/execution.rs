use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    fs,
    path::{Component, Path, PathBuf},
    process::Command,
};

use crate::utils::string_utils::StrUtils;
use crate::utils::trace_utils::ChatTraceStep;

const TOOL_TRACE_DETAIL_LIMIT: usize = 6000;
const APPLY_PATCH_TRACE_DETAIL_LIMIT: usize = 128 * 1024;

pub(crate) type ToolStreamSink<'a> = dyn FnMut(ChatTraceStep) + 'a;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ApplyPatchRequest {
    pub(crate) workspace_path: String,
    pub(crate) patch_text: String,
    pub(crate) files: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ApplyPatchResponse {
    pub(crate) applied_files: Vec<String>,
    pub(crate) stdout: String,
    pub(crate) stderr: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct InspectCodeWorkspaceRequest {
    pub(crate) workspace_path: String,
    pub(crate) query: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct InspectCodeWorkspaceResponse {
    pub(crate) tool: String,
    pub(crate) content: String,
}

pub(crate) fn trace_step_with_detail(kind: &str, text: String, detail: String) -> ChatTraceStep {
    trace_step_with_detail_limit(kind, text, detail, TOOL_TRACE_DETAIL_LIMIT)
}

pub(crate) fn trace_step_with_detail_limit(
    kind: &str,
    text: String,
    detail: String,
    detail_limit: usize,
) -> ChatTraceStep {
    ChatTraceStep {
        kind: kind.to_string(),
        text,
        detail: Some(StrUtils::truncate_text(detail, detail_limit)),
    }
}

pub(crate) fn parsed_tool_arguments(function: &Value) -> Value {
    let arguments = function
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!("{}"));

    if let Some(arguments_text) = arguments.as_str() {
        serde_json::from_str::<Value>(arguments_text)
            .or_else(|_| {
                serde_json::from_str::<Value>(&StrUtils::normalize_json_smart_quotes(
                    arguments_text,
                ))
            })
            .unwrap_or_else(|_| Value::String(arguments_text.to_string()))
    } else {
        arguments
    }
}

fn compact_trace_json(value: &Value) -> String {
    serde_json::to_string(value)
        .map(|text| StrUtils::truncate_text(text, 280))
        .unwrap_or_else(|_| "<unreadable arguments>".to_string())
}

fn pretty_trace_json(value: &Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_else(|_| "<unreadable arguments>".to_string())
}

pub(crate) fn tool_call_trace_step(tool_call: &Value) -> ChatTraceStep {
    let function = tool_call.get("function").unwrap_or(&Value::Null);
    let name = function
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("unknown_tool");
    let arguments = parsed_tool_arguments(function);
    let text = if name == "codegraph_explore" {
        let query = arguments
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let max_files = arguments.get("maxFiles").and_then(Value::as_u64);

        match max_files {
            Some(max_files) => format!(
                "codegraph_explore query=\"{}\" maxFiles={}",
                StrUtils::truncate_text(query.to_string(), 180),
                max_files
            ),
            None => format!(
                "codegraph_explore query=\"{}\"",
                StrUtils::truncate_text(query.to_string(), 200)
            ),
        }
    } else {
        format!("{} {}", name, compact_trace_json(&arguments))
    };

    let detail = format!(
        "Tool: {}\nArguments:\n{}",
        name,
        pretty_trace_json(&arguments)
    );

    let detail_limit = if name == "apply_patch" {
        APPLY_PATCH_TRACE_DETAIL_LIMIT
    } else {
        TOOL_TRACE_DETAIL_LIMIT
    };

    trace_step_with_detail_limit("tool", text, detail, detail_limit)
}

pub(crate) fn tool_result_trace_step(tool_call: &Value, tool_message: &Value) -> ChatTraceStep {
    let function = tool_call.get("function").unwrap_or(&Value::Null);
    let name = function
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("unknown_tool");
    let content = StrUtils::message_content_text(tool_message);
    let route = if content.contains("Local command fallback") || content.contains("local fallback")
    {
        "local fallback"
    } else if content.contains("CodeGraph") {
        "CodeGraph"
    } else {
        "tool"
    };
    let first_line = content
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("");
    let text = if first_line.is_empty() {
        format!("{} returned an empty result", name)
    } else {
        format!(
            "{} returned {} chars via {}: {}",
            name,
            content.chars().count(),
            route,
            StrUtils::truncate_text(first_line.to_string(), 160)
        )
    };

    let detail = if name == "run_command" {
        format!(
            "Tool: {}\nResult characters: {}\n\nFull stdout/stderr is shown in the live command output entry above.",
            name,
            content.chars().count(),
        )
    } else {
        format!(
            "Tool: {}\nResult characters: {}\n\n{}",
            name,
            content.chars().count(),
            content
        )
    };

    trace_step_with_detail("tool", text, detail)
}

pub(crate) fn execute_code_tool_call(
    workspace: &Path,
    tool_call: &Value,
    allow_writes: bool,
    stream_sink: Option<&mut ToolStreamSink<'_>>,
) -> Value {
    let tool_call_id = tool_call
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or("codegraph-tool-call");
    let function = tool_call.get("function").unwrap_or(&Value::Null);
    let name = function.get("name").and_then(Value::as_str).unwrap_or("");
    let arguments = parsed_tool_arguments(function);

    let content = if !allow_writes && is_write_code_tool(name, &arguments) {
        Err(
            "This member's write permission is disabled. Only read-only code tools are available."
                .to_string(),
        )
    } else {
        match name {
            "codegraph_explore" => {
                crate::tools::codegraph::execute_codegraph_explore_tool(workspace, &arguments)
            }
            "codegraph_command" => {
                crate::tools::codegraph::execute_codegraph_command_tool(workspace, &arguments)
            }
            "read_file" => read_workspace_file_tool(workspace, &arguments),
            "list_files" => list_workspace_files_tool(workspace, &arguments),
            "search_files" => search_workspace_files_tool(workspace, &arguments),
            "glob_files" => glob_workspace_files_tool(workspace, &arguments),
            "write_file" => write_workspace_file_tool(workspace, &arguments),
            "create_directory" => create_workspace_directory_tool(workspace, &arguments),
            "delete_path" => delete_workspace_path_tool(workspace, &arguments),
            "move_path" => move_workspace_path_tool(workspace, &arguments),
            "apply_patch" => crate::tools::patch::apply_patch_tool(workspace, &arguments),
            "run_command" => crate::tools::command::run_workspace_command_tool(
                workspace,
                &arguments,
                stream_sink,
            ),
            "dispatch_tasks" => execute_dispatch_tasks(&arguments),
            _ => Err(format!("Unknown tool: {}", name)),
        }
    };

    json!({
        "role": "tool",
        "tool_call_id": tool_call_id,
        "content": content.unwrap_or_else(|error| format!("Tool {} failed: {}", name, error)),
    })
}

fn is_write_code_tool(name: &str, arguments: &Value) -> bool {
    matches!(
        name,
        "write_file"
            | "create_directory"
            | "delete_path"
            | "move_path"
            | "apply_patch"
            | "run_command"
    ) || (name == "codegraph_command"
        && arguments
            .get("command")
            .and_then(Value::as_str)
            .is_some_and(|command| {
                crate::tools::schema::CODEGRAPH_WRITE_COMMANDS.contains(&command)
            }))
}

fn execute_dispatch_tasks(arguments: &Value) -> Result<String, String> {
    let tasks = arguments
        .get("tasks")
        .and_then(Value::as_array)
        .ok_or_else(|| "dispatch_tasks requires a non-empty \"tasks\" array.".to_string())?;

    if tasks.is_empty() {
        return Err("dispatch_tasks requires at least one task entry.".to_string());
    }

    let mut dispatched = Vec::new();
    for (index, task) in tasks.iter().enumerate() {
        let member = task
            .get("member")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .ok_or_else(|| format!("Task {} is missing a non-empty \"member\" name.", index + 1))?;
        let instruction = task
            .get("instruction")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|inst| !inst.is_empty())
            .ok_or_else(|| {
                format!(
                    "Task {} (member \"{}\") is missing a non-empty \"instruction\".",
                    index + 1,
                    member
                )
            })?;

        dispatched.push(json!({
            "member": member,
            "instruction": instruction,
        }));
    }

    Ok(format!(
        "Tasks dispatched successfully to {} member(s):\n{}",
        dispatched.len(),
        dispatched
            .iter()
            .map(|entry| format!(
                "- @{}: {}",
                entry["member"].as_str().unwrap_or("?"),
                entry["instruction"].as_str().unwrap_or("?")
            ))
            .collect::<Vec<_>>()
            .join("\n")
    ))
}

pub(crate) fn tool_arg_string<'a>(arguments: &'a Value, key: &str) -> &'a str {
    arguments
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("")
}

pub(crate) fn tool_arg_bool(arguments: &Value, key: &str, default: bool) -> bool {
    arguments
        .get(key)
        .and_then(Value::as_bool)
        .unwrap_or(default)
}

pub(crate) fn tool_arg_string_array(arguments: &Value, key: &str) -> Vec<String> {
    arguments
        .get(key)
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

pub(crate) fn tool_arg_string_map(arguments: &Value, key: &str) -> Vec<(String, String)> {
    arguments
        .get(key)
        .and_then(Value::as_object)
        .map(|values| {
            values
                .iter()
                .filter_map(|(name, value)| {
                    let name = name.trim();

                    if name.is_empty() || name.contains('=') {
                        return None;
                    }

                    value
                        .as_str()
                        .map(|value| (name.to_string(), value.to_string()))
                })
                .collect()
        })
        .unwrap_or_default()
}

pub(crate) fn tool_arg_usize(
    arguments: &Value,
    key: &str,
    default: usize,
    min: usize,
    max: usize,
) -> usize {
    arguments
        .get(key)
        .and_then(Value::as_u64)
        .map(|value| value as usize)
        .unwrap_or(default)
        .clamp(min, max)
}

pub(crate) fn resolve_workspace_relative_path(
    workspace: &Path,
    raw_path: &str,
) -> Result<PathBuf, String> {
    let normalized = raw_path.trim().replace('\\', "/");

    if normalized.is_empty() {
        return Ok(workspace.to_path_buf());
    }

    let relative = Path::new(&normalized);

    if relative.is_absolute() {
        return Err(format!("Absolute paths are not allowed: {}", normalized));
    }

    for component in relative.components() {
        if matches!(
            component,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        ) {
            return Err(format!("Path traversal is not allowed: {}", normalized));
        }
    }

    let target = fs::canonicalize(workspace.join(relative))
        .map_err(|error| format!("Path does not exist: {} ({})", normalized, error))?;

    if !target.starts_with(workspace) {
        return Err(format!("Path escapes workspace: {}", normalized));
    }

    Ok(target)
}

pub(crate) fn validate_relative_file(file: &str) -> Result<String, String> {
    let normalized = file.trim().replace('\\', "/");

    if normalized.is_empty() {
        return Err("Patch contains an empty file path.".to_string());
    }

    let path = Path::new(&normalized);

    if path.is_absolute() {
        return Err(format!("Absolute paths are not allowed: {}", normalized));
    }

    for component in path.components() {
        match component {
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(format!("Path traversal is not allowed: {}", normalized));
            }
            Component::Normal(part) => {
                let text = part.to_string_lossy().to_ascii_lowercase();
                if matches!(
                    text.as_str(),
                    ".ssh" | ".aws" | ".config" | "node_modules" | "dist" | "target"
                ) || text.starts_with(".env")
                {
                    return Err(format!(
                        "Sensitive or generated paths are not allowed: {}",
                        normalized
                    ));
                }
            }
            _ => {}
        }
    }

    Ok(normalized)
}

pub(crate) fn validate_workspace(workspace_path: &str) -> Result<PathBuf, String> {
    let trimmed = workspace_path.trim();

    if trimmed.is_empty() {
        return Err("Workspace path cannot be empty.".to_string());
    }

    let workspace = fs::canonicalize(trimmed).map_err(|error| {
        format!(
            "Workspace folder does not exist or is inaccessible: {}",
            error
        )
    })?;

    if !workspace.is_dir() {
        return Err("Workspace path must be a directory.".to_string());
    }

    Ok(workspace)
}

fn resolve_workspace_target_path(workspace: &Path, raw_path: &str) -> Result<PathBuf, String> {
    let normalized = validate_relative_file(raw_path)?;
    Ok(workspace.join(normalized))
}

fn ensure_target_stays_in_workspace(workspace: &Path, target: &Path) -> Result<(), String> {
    let existing = if target.exists() {
        target.to_path_buf()
    } else {
        target
            .parent()
            .map(Path::to_path_buf)
            .ok_or_else(|| "Target has no parent directory.".to_string())?
    };
    let canonical = fs::canonicalize(&existing)
        .map_err(|error| format!("Failed to verify {}: {}", existing.display(), error))?;

    if !canonical.starts_with(workspace) {
        return Err(format!("Path escapes workspace: {}", target.display()));
    }

    Ok(())
}

fn workspace_relative_display(workspace: &Path, path: &Path) -> String {
    path.strip_prefix(workspace)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn is_skipped_directory(path: &Path) -> bool {
    path.components().any(|component| {
        component
            .as_os_str()
            .to_str()
            .map(|part| matches!(part, ".git" | "node_modules" | "dist" | "target"))
            .unwrap_or(false)
    })
}

fn collect_workspace_files(
    workspace: &Path,
    directory: &Path,
    recursive: bool,
    files: &mut Vec<String>,
    max_results: usize,
) -> Result<(), String> {
    if files.len() >= max_results || is_skipped_directory(directory) {
        return Ok(());
    }

    let entries = fs::read_dir(directory)
        .map_err(|error| format!("Failed to list {}: {}", directory.display(), error))?;

    for entry in entries {
        if files.len() >= max_results {
            break;
        }

        let entry = entry.map_err(|error| error.to_string())?;
        let path = entry.path();

        if path.is_dir() {
            if recursive {
                collect_workspace_files(workspace, &path, recursive, files, max_results)?;
            }
            continue;
        }

        if path.is_file() {
            files.push(workspace_relative_display(workspace, &path));
        }
    }

    Ok(())
}

fn list_workspace_files_tool(workspace: &Path, arguments: &Value) -> Result<String, String> {
    let path = resolve_workspace_relative_path(workspace, tool_arg_string(arguments, "path"))?;
    let recursive = tool_arg_bool(arguments, "recursive", true);
    let max_results = tool_arg_usize(arguments, "maxResults", 120, 1, 500);

    if !path.is_dir() {
        return Err("list_files path must be a directory.".to_string());
    }

    let mut files = Vec::new();
    collect_workspace_files(workspace, &path, recursive, &mut files, max_results)?;
    files.sort();

    if files.is_empty() {
        return Ok("No files found.".to_string());
    }

    Ok(format!(
        "Files under {} (showing up to {}):\n{}",
        workspace_relative_display(workspace, &path),
        max_results,
        files.join("\n")
    ))
}

pub(crate) fn read_workspace_file_tool(
    workspace: &Path,
    arguments: &Value,
) -> Result<String, String> {
    let file = tool_arg_string(arguments, "file");

    if file.is_empty() {
        return Err("read_file requires a file path.".to_string());
    }

    let path = resolve_workspace_relative_path(workspace, file)?;

    if !path.is_file() {
        return Err(format!("Not a file: {}", file));
    }

    let metadata = fs::metadata(&path).map_err(|error| error.to_string())?;

    if metadata.len() > 5_000_000 {
        return Err("File is too large to read directly; use search_files first.".to_string());
    }

    let start_line = tool_arg_usize(arguments, "startLine", 1, 1, usize::MAX);
    let max_lines = tool_arg_usize(arguments, "maxLines", 240, 1, 1000);
    let content =
        fs::read_to_string(&path).map_err(|error| format!("Failed to read {}: {}", file, error))?;
    crate::tools::read_guard::record_read(&path, &content);
    let lines: Vec<&str> = content.lines().collect();

    if start_line > lines.len().max(1) {
        return Ok(format!(
            "{} has {} lines; startLine {} is past the end.",
            workspace_relative_display(workspace, &path),
            lines.len(),
            start_line
        ));
    }

    let start_index = start_line.saturating_sub(1);
    let end_index = (start_index + max_lines).min(lines.len());
    let numbered = lines[start_index..end_index]
        .iter()
        .enumerate()
        .map(|(index, line)| format!("{}\t{}", start_index + index + 1, line))
        .collect::<Vec<_>>()
        .join("\n");

    Ok(format!(
        "{} lines {}-{} of {}:\n{}",
        workspace_relative_display(workspace, &path),
        start_line,
        end_index,
        lines.len(),
        StrUtils::truncate_text(numbered, 18_000)
    ))
}

fn run_rg_files(workspace: &Path, args: &[String]) -> Result<String, String> {
    let output = Command::new("rg")
        .current_dir(workspace)
        .args(args)
        .output()
        .map_err(|error| format!("Failed to run rg: {}", error))?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    if !output.status.success() && output.status.code() != Some(1) && stdout.trim().is_empty() {
        return Err(if stderr.trim().is_empty() {
            "rg returned no usable result.".to_string()
        } else {
            stderr.trim().to_string()
        });
    }

    Ok(stdout)
}

fn rg_exclude_args() -> Vec<String> {
    vec![
        "--glob".to_string(),
        "!node_modules".to_string(),
        "--glob".to_string(),
        "!dist".to_string(),
        "--glob".to_string(),
        "!target".to_string(),
        "--glob".to_string(),
        "!.git".to_string(),
    ]
}

fn search_workspace_files_tool(workspace: &Path, arguments: &Value) -> Result<String, String> {
    let query = tool_arg_string(arguments, "query");

    if query.is_empty() {
        return Err("search_files requires a query.".to_string());
    }

    let max_results = tool_arg_usize(arguments, "maxResults", 80, 1, 300);
    let mut args = vec![
        "--line-number".to_string(),
        "--no-heading".to_string(),
        "--color".to_string(),
        "never".to_string(),
    ];
    args.extend(rg_exclude_args());

    if !tool_arg_bool(arguments, "caseSensitive", false) {
        args.push("--ignore-case".to_string());
    }

    if tool_arg_bool(arguments, "literal", false) {
        args.push("--fixed-strings".to_string());
    }

    args.push(query.to_string());

    let path_arg = tool_arg_string(arguments, "path");
    if !path_arg.is_empty() {
        let path = resolve_workspace_relative_path(workspace, path_arg)?;
        args.push(workspace_relative_display(workspace, &path));
    }

    let stdout = run_rg_files(workspace, &args)?;
    let lines = stdout
        .lines()
        .filter(|line| !line.trim().is_empty())
        .take(max_results)
        .collect::<Vec<_>>();

    if lines.is_empty() {
        return Ok(format!("No matches for `{}`.", query));
    }

    Ok(format!(
        "Matches for `{}` (showing up to {}):\n{}",
        query,
        max_results,
        StrUtils::truncate_text(lines.join("\n"), 18_000)
    ))
}

fn glob_workspace_files_tool(workspace: &Path, arguments: &Value) -> Result<String, String> {
    let pattern = tool_arg_string(arguments, "pattern");

    if pattern.is_empty() {
        return Err("glob_files requires a pattern.".to_string());
    }

    let max_results = tool_arg_usize(arguments, "maxResults", 120, 1, 500);
    let mut args = vec!["--files".to_string()];
    args.extend(rg_exclude_args());
    args.push("--glob".to_string());
    args.push(pattern.to_string());

    let path_arg = tool_arg_string(arguments, "path");
    if !path_arg.is_empty() {
        let path = resolve_workspace_relative_path(workspace, path_arg)?;
        args.push(workspace_relative_display(workspace, &path));
    }

    let stdout = run_rg_files(workspace, &args)?;
    let files = stdout
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .take(max_results)
        .collect::<Vec<_>>();

    if files.is_empty() {
        return Ok(format!("No files matched `{}`.", pattern));
    }

    Ok(format!(
        "Files matching `{}` (showing up to {}):\n{}",
        pattern,
        max_results,
        files.join("\n")
    ))
}

fn ensure_parent_directory(path: &Path, create_parents: bool) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "Target path has no parent directory.".to_string())?;

    if parent.exists() {
        return Ok(());
    }

    if !create_parents {
        return Err(format!(
            "Parent directory does not exist: {}",
            parent.display()
        ));
    }

    fs::create_dir_all(parent)
        .map_err(|error| format!("Failed to create {}: {}", parent.display(), error))
}

pub(crate) fn write_workspace_file_tool(
    workspace: &Path,
    arguments: &Value,
) -> Result<String, String> {
    let file = tool_arg_string(arguments, "file");
    let content = arguments
        .get("content")
        .and_then(Value::as_str)
        .unwrap_or("");
    let mode = tool_arg_string(arguments, "mode");
    let create_parents = tool_arg_bool(arguments, "createParents", true);

    if file.is_empty() {
        return Err("write_file requires a file path.".to_string());
    }

    if content.len() > 2_000_000 {
        return Err("write_file content is too large.".to_string());
    }

    let path = resolve_workspace_target_path(workspace, file)?;
    ensure_parent_directory(&path, create_parents)?;
    ensure_target_stays_in_workspace(workspace, &path)?;

    // "create" has its own existence check; every other write must prove a fresh read.
    if mode != "create" {
        crate::tools::read_guard::check_before_edit(&path, file)?;
    }

    match mode {
        "" | "overwrite" => {
            fs::write(&path, content)
                .map_err(|error| format!("Failed to write {}: {}", file, error))?;
        }
        "create" => {
            if path.exists() {
                return Err(format!("File already exists: {}", file));
            }
            fs::write(&path, content)
                .map_err(|error| format!("Failed to create {}: {}", file, error))?;
        }
        "append" => {
            let mut file_handle = fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                .map_err(|error| format!("Failed to open {}: {}", file, error))?;
            use std::io::Write;
            file_handle
                .write_all(content.as_bytes())
                .map_err(|error| format!("Failed to append {}: {}", file, error))?;
        }
        other => return Err(format!("Unsupported write_file mode: {}", other)),
    }

    if let Ok(written) = fs::read_to_string(&path) {
        crate::tools::read_guard::record_write(&path, &written);
    }

    Ok(format!(
        "{} {} ({} bytes).",
        match mode {
            "append" => "Appended",
            "create" => "Created",
            _ => "Wrote",
        },
        workspace_relative_display(workspace, &path),
        content.len()
    ))
}

fn create_workspace_directory_tool(workspace: &Path, arguments: &Value) -> Result<String, String> {
    let path_arg = tool_arg_string(arguments, "path");

    if path_arg.is_empty() {
        return Err("create_directory requires a path.".to_string());
    }

    let path = resolve_workspace_target_path(workspace, path_arg)?;
    fs::create_dir_all(&path)
        .map_err(|error| format!("Failed to create {}: {}", path_arg, error))?;
    ensure_target_stays_in_workspace(workspace, &path)?;

    Ok(format!(
        "Created directory {}.",
        workspace_relative_display(workspace, &path)
    ))
}

pub(crate) fn delete_workspace_path_tool(
    workspace: &Path,
    arguments: &Value,
) -> Result<String, String> {
    let path_arg = tool_arg_string(arguments, "path");
    let recursive = tool_arg_bool(arguments, "recursive", false);

    if path_arg.is_empty() {
        return Err("delete_path requires a path.".to_string());
    }

    let validated_path = validate_relative_file(path_arg)?;
    let path = resolve_workspace_relative_path(workspace, &validated_path)?;

    if path == workspace {
        return Err("delete_path refuses to delete the workspace root.".to_string());
    }

    if path.is_dir() {
        if !recursive {
            return Err("delete_path requires recursive=true for directories.".to_string());
        }
        fs::remove_dir_all(&path)
            .map_err(|error| format!("Failed to delete directory {}: {}", path_arg, error))?;
    } else {
        fs::remove_file(&path)
            .map_err(|error| format!("Failed to delete file {}: {}", path_arg, error))?;
    }

    Ok(format!("Deleted {}.", path_arg))
}

pub(crate) fn move_workspace_path_tool(
    workspace: &Path,
    arguments: &Value,
) -> Result<String, String> {
    let from = tool_arg_string(arguments, "from");
    let to = tool_arg_string(arguments, "to");
    let create_parents = tool_arg_bool(arguments, "createParents", true);

    if from.is_empty() || to.is_empty() {
        return Err("move_path requires both from and to.".to_string());
    }

    let validated_source = validate_relative_file(from)?;
    let source = resolve_workspace_relative_path(workspace, &validated_source)?;
    let target = resolve_workspace_target_path(workspace, to)?;

    if source == workspace {
        return Err("move_path refuses to move the workspace root.".to_string());
    }

    ensure_parent_directory(&target, create_parents)?;
    ensure_target_stays_in_workspace(workspace, &target)?;
    fs::rename(&source, &target)
        .map_err(|error| format!("Failed to move {} to {}: {}", from, to, error))?;

    Ok(format!("Moved {} to {}.", from, to))
}
