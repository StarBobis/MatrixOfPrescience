use serde_json::Value;
use std::{
    env, fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

use crate::tools::execution::{
    tool_arg_bool, tool_arg_string, tool_arg_string_array, validate_workspace,
    InspectCodeWorkspaceRequest, InspectCodeWorkspaceResponse,
};
use crate::utils::string_utils::StrUtils;

pub(crate) const DEFAULT_CODEGRAPH_MAX_FILES: u64 = 12;
pub(crate) const MAX_CODEGRAPH_MAX_FILES: u64 = 24;
const CODEGRAPH_EXPLORE_SCOPE_NOTE: &str = "CodeGraph explore note: `Found N symbols across M files` describes only this query's returned relevant symbols/files. It is not the total CodeGraph index file count, and should not be used as the index health/status summary. If a CodeGraph index status section is present, use that for status questions.";

pub(crate) fn normalize_codegraph_max_files(max_files: Option<u64>) -> u64 {
    max_files
        .unwrap_or(DEFAULT_CODEGRAPH_MAX_FILES)
        .clamp(1, MAX_CODEGRAPH_MAX_FILES)
}

fn contains_any(text: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| text.contains(needle))
}

pub(crate) fn is_codegraph_status_query(query: &str) -> bool {
    let lower = query.to_lowercase();

    contains_any(
        &lower,
        &[
            "codegraph status",
            "status",
            "health",
            "statistics",
            "stats",
            "coverage",
            "indexed files",
            "index coverage",
            "index statistics",
            "状态",
            "健康",
            "统计",
            "覆盖",
            "工作正常",
            "文件数",
        ],
    ) || (lower.contains("index")
        && contains_any(
            &lower,
            &[
                "up to date",
                "file count",
                "files indexed",
                "how many files",
                "total files",
            ],
        ))
        || (lower.contains("索引")
            && contains_any(
                &lower,
                &["状态", "统计", "覆盖", "文件", "正常", "健康", "多少"],
            ))
}

pub(crate) fn format_codegraph_explore_output(
    explore_output: &str,
    status_output: Option<&str>,
) -> String {
    let mut sections = vec![CODEGRAPH_EXPLORE_SCOPE_NOTE.to_string()];

    if let Some(status) = status_output
        .map(str::trim)
        .filter(|status| !status.is_empty())
    {
        sections.push(format!("CodeGraph index status:\n{}", status));
    }

    sections.push(format!(
        "CodeGraph explore result:\n{}",
        explore_output.trim()
    ));
    sections.join("\n\n")
}

fn has_codegraph_index(workspace: &Path) -> bool {
    workspace
        .ancestors()
        .any(|path| path.join(".codegraph").is_dir())
}

fn codegraph_command_candidates() -> Vec<PathBuf> {
    let mut candidates = vec![PathBuf::from("codegraph"), PathBuf::from("codegraph.cmd")];

    if let Ok(app_data) = env::var("APPDATA") {
        candidates.push(PathBuf::from(app_data).join("npm").join("codegraph.cmd"));
    }

    if let Ok(user_profile) = env::var("USERPROFILE") {
        candidates.push(
            PathBuf::from(user_profile)
                .join("AppData")
                .join("Roaming")
                .join("npm")
                .join("codegraph.cmd"),
        );
    }

    candidates
}

fn run_script_command(script: &Path, workspace: &Path, args: &[&str]) -> Result<Output, String> {
    let extension = script
        .extension()
        .and_then(|value| value.to_str())
        .map(str::to_ascii_lowercase);

    if matches!(extension.as_deref(), Some("cmd" | "bat")) {
        let mut command = Command::new("cmd");
        command.current_dir(workspace).arg("/C").arg(script);
        for arg in args {
            command.arg(arg);
        }
        return command.output().map_err(|error| error.to_string());
    }

    let mut command = Command::new(script);
    command.current_dir(workspace);
    for arg in args {
        command.arg(arg);
    }
    command.output().map_err(|error| error.to_string())
}

fn run_codegraph_command(workspace: &Path, args: &[&str]) -> Result<Output, String> {
    let mut errors = Vec::new();

    for candidate in codegraph_command_candidates() {
        let output = run_script_command(&candidate, workspace, args);

        match output {
            Ok(output) => return Ok(output),
            Err(error) => errors.push(format!("{}: {}", candidate.display(), error)),
        }
    }

    Err(format!(
        "Failed to start CodeGraph. Tried: {}",
        errors.join("; ")
    ))
}

fn run_codegraph_status(workspace: &Path) -> Result<String, String> {
    let output = run_codegraph_command(workspace, &["status"])?;
    let stdout = StrUtils::strip_ansi_escape_sequences(&String::from_utf8_lossy(&output.stdout));
    let stderr = StrUtils::strip_ansi_escape_sequences(&String::from_utf8_lossy(&output.stderr));

    if !output.status.success() || stdout.trim().is_empty() {
        return Err(if stderr.trim().is_empty() {
            "CodeGraph status returned no usable result.".to_string()
        } else {
            format!("CodeGraph status failed: {}", stderr.trim())
        });
    }

    Ok(stdout.trim().to_string())
}

pub(crate) fn run_codegraph_explore(
    workspace: &Path,
    query: &str,
    max_files: Option<u64>,
) -> Result<String, String> {
    if !has_codegraph_index(workspace) {
        return Err(format!(
            "No .codegraph index was found for {}. Select an indexed workspace or run `codegraph init` and `codegraph index` in that project.",
            workspace.display()
        ));
    }

    let max_files_arg = normalize_codegraph_max_files(max_files).to_string();
    let output = run_codegraph_command(
        workspace,
        &["explore", "--max-files", max_files_arg.as_str(), query],
    )?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    if !output.status.success() || stdout.trim().is_empty() {
        return Err(if stderr.trim().is_empty() {
            "CodeGraph returned no usable result.".to_string()
        } else {
            format!("CodeGraph query failed: {}", stderr.trim())
        });
    }

    let status_output =
        if is_codegraph_status_query(query) {
            Some(run_codegraph_status(workspace).unwrap_or_else(|error| {
                format!("CodeGraph index status could not be read: {}", error)
            }))
        } else {
            None
        };

    Ok(crate::utils::spill::spill_tool_output(
        workspace,
        "codegraph-explore",
        format_codegraph_explore_output(&stdout, status_output.as_deref()),
        18_000,
    ))
}

fn codegraph_required_arg(arguments: &Value, key: &str, command: &str) -> Result<String, String> {
    let value = tool_arg_string(arguments, key);
    if value.is_empty() {
        Err(format!(
            "codegraph_command requires `{}` when command is `{}`.",
            key, command
        ))
    } else {
        Ok(value.to_string())
    }
}

fn push_codegraph_number_arg(
    args: &mut Vec<String>,
    arguments: &Value,
    key: &str,
    flag: &str,
    maximum: u64,
) {
    if let Some(value) = arguments.get(key).and_then(Value::as_u64) {
        args.push(flag.to_string());
        args.push(value.clamp(1, maximum).to_string());
    }
}

fn push_codegraph_string_arg(args: &mut Vec<String>, value: &str, flag: &str) {
    if !value.is_empty() {
        args.push(flag.to_string());
        args.push(value.to_string());
    }
}

fn build_codegraph_command_args(arguments: &Value) -> Result<(Vec<String>, bool), String> {
    let command = tool_arg_string(arguments, "command");
    let mut args = vec![command.to_string()];
    let mut requires_index = true;

    match command {
        "status" => {
            if tool_arg_bool(arguments, "json", false) {
                args.push("--json".to_string());
            }
        }
        "query" => {
            push_codegraph_number_arg(&mut args, arguments, "limit", "--limit", 200);
            push_codegraph_string_arg(&mut args, tool_arg_string(arguments, "kind"), "--kind");
            if tool_arg_bool(arguments, "json", false) {
                args.push("--json".to_string());
            }
            args.push(codegraph_required_arg(arguments, "query", command)?);
        }
        "node" => {
            let symbol = tool_arg_string(arguments, "symbol");
            let file = tool_arg_string(arguments, "file");
            if symbol.is_empty() && file.is_empty() {
                return Err(
                    "codegraph_command requires `symbol` or `file` when command is `node`."
                        .to_string(),
                );
            }
            push_codegraph_string_arg(&mut args, file, "--file");
            push_codegraph_number_arg(&mut args, arguments, "offset", "--offset", u64::MAX);
            push_codegraph_number_arg(&mut args, arguments, "limit", "--limit", 2_000);
            if tool_arg_bool(arguments, "symbolsOnly", false) {
                args.push("--symbols-only".to_string());
            }
            if !symbol.is_empty() {
                args.push(symbol.to_string());
            }
        }
        "files" => {
            push_codegraph_string_arg(&mut args, tool_arg_string(arguments, "filter"), "--filter");
            push_codegraph_string_arg(
                &mut args,
                tool_arg_string(arguments, "pattern"),
                "--pattern",
            );
            push_codegraph_string_arg(&mut args, tool_arg_string(arguments, "format"), "--format");
            push_codegraph_number_arg(&mut args, arguments, "maxDepth", "--max-depth", 20);
            if tool_arg_bool(arguments, "json", false) {
                args.push("--json".to_string());
            }
        }
        "callers" | "callees" => {
            push_codegraph_number_arg(&mut args, arguments, "limit", "--limit", 200);
            if tool_arg_bool(arguments, "json", false) {
                args.push("--json".to_string());
            }
            args.push(codegraph_required_arg(arguments, "symbol", command)?);
        }
        "impact" => {
            push_codegraph_number_arg(&mut args, arguments, "depth", "--depth", 20);
            if tool_arg_bool(arguments, "json", false) {
                args.push("--json".to_string());
            }
            args.push(codegraph_required_arg(arguments, "symbol", command)?);
        }
        "affected" => {
            push_codegraph_number_arg(&mut args, arguments, "depth", "--depth", 20);
            push_codegraph_string_arg(&mut args, tool_arg_string(arguments, "filter"), "--filter");
            if tool_arg_bool(arguments, "json", false) {
                args.push("--json".to_string());
            }
            args.extend(tool_arg_string_array(arguments, "files"));
        }
        "init" | "unlock" => requires_index = false,
        "index" => {
            requires_index = false;
            args.push("--quiet".to_string());
        }
        "sync" => args.push("--quiet".to_string()),
        _ => {
            return Err(format!(
                "Unsupported CodeGraph command `{}`. Allowed commands: read commands and write commands.",
                command
            ))
        }
    }

    Ok((args, requires_index))
}

pub(crate) fn execute_codegraph_explore_tool(
    workspace: &Path,
    arguments: &Value,
) -> Result<String, String> {
    let query = arguments
        .get("query")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let max_files = arguments.get("maxFiles").and_then(Value::as_u64);

    if query.trim().is_empty() {
        return Err("CodeGraph query cannot be empty.".to_string());
    }

    match run_codegraph_explore(workspace, query, max_files) {
        Ok(content) => Ok(content),
        Err(codegraph_error) => read_local_code_context(workspace, query)
            .map(|fallback| {
                format!(
                    "CodeGraph tool was called, but CodeGraph execution failed and local fallback was used.\nCodeGraph error: {}\n\n{}",
                    codegraph_error, fallback
                )
            })
            .map_err(|fallback_error| {
                format!(
                    "CodeGraph could not read this workspace: {}. Local fallback failed: {}",
                    codegraph_error, fallback_error
                )
            }),
    }
}

pub(crate) fn execute_codegraph_command_tool(
    workspace: &Path,
    arguments: &Value,
) -> Result<String, String> {
    let (args, requires_index) = build_codegraph_command_args(arguments)?;
    let command = args.first().map(String::as_str).unwrap_or("command");

    if requires_index && !has_codegraph_index(workspace) {
        return Err(format!(
            "No .codegraph index was found for {}. Run codegraph_command with command `init` first.",
            workspace.display()
        ));
    }

    let arg_refs = args.iter().map(String::as_str).collect::<Vec<_>>();
    let output = run_codegraph_command(workspace, &arg_refs)?;
    let stdout = StrUtils::strip_ansi_escape_sequences(&String::from_utf8_lossy(&output.stdout));
    let stderr = StrUtils::strip_ansi_escape_sequences(&String::from_utf8_lossy(&output.stderr));
    let detail = match (stdout.trim().is_empty(), stderr.trim().is_empty()) {
        (false, false) => format!("stdout:\n{}\n\nstderr:\n{}", stdout.trim(), stderr.trim()),
        (false, true) => stdout.trim().to_string(),
        (true, false) => stderr.trim().to_string(),
        (true, true) => format!("CodeGraph {} completed successfully.", command),
    };

    if !output.status.success() {
        let code = output
            .status
            .code()
            .map(|value| value.to_string())
            .unwrap_or_else(|| "terminated".to_string());
        return Err(format!(
            "CodeGraph {} failed with exit code {}:\n{}",
            command, code, detail
        ));
    }

    Ok(crate::utils::spill::spill_tool_output(
        workspace,
        "codegraph-command",
        format!("CodeGraph {} result:\n{}", command, detail),
        20_000,
    ))
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

pub(crate) fn read_local_code_context(workspace: &Path, query: &str) -> Result<String, String> {
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
        .map_err(|error| format!("Failed to read local file list: {}", error))?;

    if !output.status.success() {
        return Err("Failed to read local file list.".to_string());
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
        "Local command fallback content follows. This is not a CodeGraph result and has no symbol graph analysis.".to_string(),
        "File list:".to_string(),
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
                StrUtils::ellipsis_text(content, 4_000)
            ));
        }
    }

    Ok(crate::utils::spill::spill_tool_output(
        workspace,
        "codegraph-context",
        sections.join("\n"),
        18_000,
    ))
}

#[tauri::command]
pub(crate) async fn inspect_code_workspace(
    request: InspectCodeWorkspaceRequest,
) -> Result<InspectCodeWorkspaceResponse, String> {
    let workspace = validate_workspace(&request.workspace_path)?;
    let query = request.query.trim();

    if query.is_empty() {
        return Err("Code inspection query cannot be empty.".to_string());
    }

    match run_codegraph_explore(&workspace, query, None) {
        Ok(content) => Ok(InspectCodeWorkspaceResponse {
            tool: "CodeGraph".to_string(),
            content,
        }),
        Err(codegraph_error) => {
            let fallback = read_local_code_context(&workspace, query)?;
            Ok(InspectCodeWorkspaceResponse {
                tool: format!("LocalCommands (CodeGraph failed: {})", codegraph_error),
                content: fallback,
            })
        }
    }
}
