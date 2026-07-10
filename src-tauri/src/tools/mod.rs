use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    env, fs,
    io::{Read, Write},
    path::{Component, Path, PathBuf},
    process::{Command, Output, Stdio},
    sync::mpsc::{self, RecvTimeoutError, Sender},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use crate::{message_content_text, ChatTraceStep};

const TRACE_STEP_TEXT_LIMIT: usize = 280;
const TOOL_TRACE_DETAIL_LIMIT: usize = 6000;
const APPLY_PATCH_TRACE_DETAIL_LIMIT: usize = 128 * 1024;
const COMMAND_STREAM_CHUNK_SIZE: usize = 4096;

pub(crate) type ToolStreamSink<'a> = dyn FnMut(ChatTraceStep) + 'a;

#[derive(Clone, Copy, Debug)]
enum CommandOutputStream {
    Stdout,
    Stderr,
}

#[derive(Debug)]
enum CommandOutputEvent {
    Chunk(CommandOutputStream, String),
    ReadError(CommandOutputStream, String),
    Done(CommandOutputStream),
}

impl CommandOutputStream {
    fn label(self) -> &'static str {
        match self {
            Self::Stdout => "stdout",
            Self::Stderr => "stderr",
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ApplyPatchRequest {
    workspace_path: String,
    patch_text: String,
    files: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ApplyPatchResponse {
    applied_files: Vec<String>,
    stdout: String,
    stderr: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct InspectCodeWorkspaceRequest {
    workspace_path: String,
    query: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct InspectCodeWorkspaceResponse {
    tool: String,
    content: String,
}

fn trace_step_with_detail(kind: &str, text: String, detail: String) -> ChatTraceStep {
    trace_step_with_detail_limit(kind, text, detail, TOOL_TRACE_DETAIL_LIMIT)
}

fn trace_step_with_detail_limit(
    kind: &str,
    text: String,
    detail: String,
    detail_limit: usize,
) -> ChatTraceStep {
    ChatTraceStep {
        kind: kind.to_string(),
        text,
        detail: Some(truncate_text(detail, detail_limit)),
    }
}

fn parsed_tool_arguments(function: &Value) -> Value {
    let arguments = function
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!("{}"));

    if let Some(arguments_text) = arguments.as_str() {
        serde_json::from_str::<Value>(arguments_text)
            .or_else(|_| {
                serde_json::from_str::<Value>(&normalize_json_smart_quotes(arguments_text))
            })
            .unwrap_or_else(|_| Value::String(arguments_text.to_string()))
    } else {
        arguments
    }
}

fn normalize_json_smart_quotes(text: &str) -> String {
    let mut normalized = String::with_capacity(text.len());
    let mut in_ascii_string = false;
    let mut escaped = false;

    for ch in text.chars() {
        if in_ascii_string {
            normalized.push(ch);

            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_ascii_string = false;
            }
            continue;
        }

        match ch {
            '"' => {
                in_ascii_string = true;
                normalized.push(ch);
            }
            '\u{201c}' | '\u{201d}' => normalized.push('"'),
            _ => normalized.push(ch),
        }
    }

    normalized
}

fn compact_trace_json(value: &Value) -> String {
    serde_json::to_string(value)
        .map(|text| truncate_text(text, TRACE_STEP_TEXT_LIMIT))
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
                truncate_text(query.to_string(), 180),
                max_files
            ),
            None => format!(
                "codegraph_explore query=\"{}\"",
                truncate_text(query.to_string(), 200)
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
    let content = message_content_text(tool_message);
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
            truncate_text(first_line.to_string(), 160)
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

fn finalize_tool_function(mut function: Value, strict: bool) -> Value {
    if strict {
        function["strict"] = json!(true);
        function["parameters"]["additionalProperties"] = json!(false);
    }

    json!({
        "type": "function",
        "function": function
    })
}

const CODEGRAPH_READ_COMMANDS: &[&str] = &[
    "status", "query", "node", "files", "callers", "callees", "impact", "affected",
];
const CODEGRAPH_WRITE_COMMANDS: &[&str] = &["init", "index", "sync", "unlock"];

fn codegraph_command_tool_schema(strict: bool, allow_writes: bool) -> Value {
    let mut commands = CODEGRAPH_READ_COMMANDS.to_vec();
    if allow_writes {
        commands.extend_from_slice(CODEGRAPH_WRITE_COMMANDS);
    }

    finalize_tool_function(
        json!({
            "name": "codegraph_command",
            "description": "Run a supported CodeGraph CLI command in the current workspace. Use status/query/node/files/callers/callees/impact/affected for focused graph reads. Use init/index/sync/unlock to create, rebuild, refresh, or repair the index when write permission allows it. Destructive uninit and global install/upgrade/daemon commands are not available.",
            "parameters": {
                "type": "object",
                "properties": {
                    "command": {
                        "type": "string",
                        "enum": commands,
                        "description": "CodeGraph subcommand to run."
                    },
                    "query": {
                        "type": "string",
                        "description": "Required search text for query."
                    },
                    "symbol": {
                        "type": "string",
                        "description": "Required symbol for callers, callees, or impact; optional symbol name for node."
                    },
                    "file": {
                        "type": "string",
                        "description": "Optional indexed file path for node file mode or symbol disambiguation."
                    },
                    "files": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Changed source files used by affected."
                    },
                    "limit": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": 200,
                        "description": "Maximum results for query, callers, callees, or node file lines."
                    },
                    "kind": {
                        "type": "string",
                        "description": "Optional symbol kind filter for query, such as function or class."
                    },
                    "depth": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": 20,
                        "description": "Dependency traversal depth for impact or affected."
                    },
                    "filter": {
                        "type": "string",
                        "description": "Directory filter for files, or test glob filter for affected."
                    },
                    "pattern": {
                        "type": "string",
                        "description": "Optional glob pattern for files."
                    },
                    "format": {
                        "type": "string",
                        "enum": ["tree", "flat", "grouped"],
                        "description": "Output format for files. Defaults to tree."
                    },
                    "maxDepth": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": 20,
                        "description": "Maximum directory depth for files tree output."
                    },
                    "offset": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "1-based starting line for node file mode."
                    },
                    "symbolsOnly": {
                        "type": "boolean",
                        "description": "For node file mode, return only the symbol map and dependents."
                    },
                    "json": {
                        "type": "boolean",
                        "description": "Request JSON output when supported."
                    }
                },
                "required": ["command"]
            }
        }),
        strict,
    )
}

pub(crate) fn code_tools_schema(strict: bool, allow_writes: bool) -> Value {
    let mut tools = vec![
        finalize_tool_function(
            json!({
                "name": "codegraph_explore",
                "description": "Read the current workspace with CodeGraph for symbols, responsibilities, and call paths. The `Found N symbols across M files` line is query-scoped, not the total index file count.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "query": {
                            "type": "string",
                            "description": "The symbols, files, call flow, or implementation question to inspect."
                        },
                        "maxFiles": {
                            "type": "integer",
                            "description": "Optional maximum number of files to include source from. Defaults to 12 and is capped at 24.",
                            "minimum": 1,
                            "maximum": 24
                        }
                    },
                    "required": ["query"]
                }
            }),
            strict,
        ),
        codegraph_command_tool_schema(strict, allow_writes),
        finalize_tool_function(
            json!({
                "name": "read_file",
                "description": "Read exact file contents from the workspace with line numbers. Use this when CodeGraph output omits implementation details.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "file": {
                            "type": "string",
                            "description": "Workspace-relative file path."
                        },
                        "startLine": {
                            "type": "integer",
                            "description": "1-based line to start from. Defaults to 1.",
                            "minimum": 1
                        },
                        "maxLines": {
                            "type": "integer",
                            "description": "Maximum lines to read. Defaults to 240 and is capped at 1000.",
                            "minimum": 1,
                            "maximum": 1000
                        }
                    },
                    "required": ["file"]
                }
            }),
            strict,
        ),
        finalize_tool_function(
            json!({
                "name": "list_files",
                "description": "List files under the workspace or a subdirectory. Use this to discover nearby files before reading them.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "Optional workspace-relative directory path."
                        },
                        "recursive": {
                            "type": "boolean",
                            "description": "Whether to recurse. Defaults to true."
                        },
                        "maxResults": {
                            "type": "integer",
                            "description": "Maximum files to return. Defaults to 120 and is capped at 500.",
                            "minimum": 1,
                            "maximum": 500
                        }
                    },
                    "required": []
                }
            }),
            strict,
        ),
        finalize_tool_function(
            json!({
                "name": "search_files",
                "description": "Search text in workspace files with ripgrep-style output. Use for finding identifiers, errors, strings, and TODOs.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "query": {
                            "type": "string",
                            "description": "Regex or literal text to search for."
                        },
                        "path": {
                            "type": "string",
                            "description": "Optional workspace-relative path to search in."
                        },
                        "caseSensitive": {
                            "type": "boolean",
                            "description": "Defaults to false."
                        },
                        "literal": {
                            "type": "boolean",
                            "description": "Treat query as fixed text instead of regex. Defaults to false."
                        },
                        "maxResults": {
                            "type": "integer",
                            "description": "Maximum matches to return. Defaults to 80 and is capped at 300.",
                            "minimum": 1,
                            "maximum": 300
                        }
                    },
                    "required": ["query"]
                }
            }),
            strict,
        ),
        finalize_tool_function(
            json!({
                "name": "glob_files",
                "description": "Find files by glob pattern, for example `src/**/*.vue` or `**/*.rs`.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "pattern": {
                            "type": "string",
                            "description": "Glob pattern relative to the workspace."
                        },
                        "path": {
                            "type": "string",
                            "description": "Optional workspace-relative directory to search from."
                        },
                        "maxResults": {
                            "type": "integer",
                            "description": "Maximum files to return. Defaults to 120 and is capped at 500.",
                            "minimum": 1,
                            "maximum": 500
                        }
                    },
                    "required": ["pattern"]
                }
            }),
            strict,
        ),
    ];

    if allow_writes {
        tools.extend([
        finalize_tool_function(
            json!({
                "name": "write_file",
                "description": "Create, overwrite, or append to a UTF-8 text file inside the workspace.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "file": {
                            "type": "string",
                            "description": "Workspace-relative file path."
                        },
                        "content": {
                            "type": "string",
                            "description": "Text content to write."
                        },
                        "mode": {
                            "type": "string",
                            "enum": ["overwrite", "create", "append"],
                            "description": "Write mode. Defaults to overwrite."
                        },
                        "createParents": {
                            "type": "boolean",
                            "description": "Whether to create missing parent directories. Defaults to true."
                        }
                    },
                    "required": ["file", "content"]
                }
            }),
            strict
        ),
        finalize_tool_function(
            json!({
                "name": "create_directory",
                "description": "Create a directory inside the workspace, including missing parents.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "Workspace-relative directory path."
                        }
                    },
                    "required": ["path"]
                }
            }),
            strict
        ),
        finalize_tool_function(
            json!({
                "name": "delete_path",
                "description": "Delete a workspace-relative file or, with recursive=true, a directory. Refuses workspace root and sensitive/generated paths.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "Workspace-relative path to delete."
                        },
                        "recursive": {
                            "type": "boolean",
                            "description": "Required for deleting directories. Defaults to false."
                        }
                    },
                    "required": ["path"]
                }
            }),
            strict
        ),
        finalize_tool_function(
            json!({
                "name": "move_path",
                "description": "Move or rename a file or directory inside the workspace.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "from": {
                            "type": "string",
                            "description": "Existing workspace-relative source path."
                        },
                        "to": {
                            "type": "string",
                            "description": "Workspace-relative destination path."
                        },
                        "createParents": {
                            "type": "boolean",
                            "description": "Whether to create missing destination parent directories. Defaults to true."
                        }
                    },
                    "required": ["from", "to"]
                }
            }),
            strict
        ),
        finalize_tool_function(
            json!({
                "name": "apply_patch",
                "description": "Apply a unified diff patch inside the workspace. Keep each call focused on one file and one coherent change; split large edits across multiple calls instead of rewriting a whole file. Every hunk body line must start with ' ', '+', '-', or '\\'. Hunk line counts must match the body. Use checkOnly=true to validate without changing files.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "patchText": {
                            "type": "string",
                            "description": "Unified diff text accepted by git apply. Context lines require a leading marker space before their source indentation."
                        },
                        "checkOnly": {
                            "type": "boolean",
                            "description": "Validate only without applying. Defaults to false."
                        }
                    },
                    "required": ["patchText"]
                }
            }),
            strict
        ),
        finalize_tool_function(
            json!({
                "name": "run_command",
                "description": "Run a non-interactive command in the workspace, such as tests, formatters, or git diff. Prefer command plus args instead of shell syntax.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "command": {
                            "type": "string",
                            "description": "Executable name, for example npm, cargo, git, rg, node, or python."
                        },
                        "args": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "Command arguments."
                        },
                        "cwd": {
                            "type": "string",
                            "description": "Optional workspace-relative working directory."
                        },
                        "timeoutMs": {
                            "type": "integer",
                            "description": "Timeout in milliseconds. Defaults to 30000 and is capped at 120000.",
                            "minimum": 1000,
                            "maximum": 120000
                        },
                        "env": {
                            "type": "object",
                            "additionalProperties": { "type": "string" },
                            "description": "Optional environment variable overrides for the child process. The command still inherits the Matrix app process environment first."
                        }
                    },
                    "required": ["command"]
                }
            }),
            strict
        )
        ]);
    }

    json!(tools)
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
            .is_some_and(|command| CODEGRAPH_WRITE_COMMANDS.contains(&command)))
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
            "codegraph_explore" => execute_codegraph_explore_tool(workspace, &arguments),
            "codegraph_command" => execute_codegraph_command_tool(workspace, &arguments),
            "read_file" => read_workspace_file_tool(workspace, &arguments),
            "list_files" => list_workspace_files_tool(workspace, &arguments),
            "search_files" => search_workspace_files_tool(workspace, &arguments),
            "glob_files" => glob_workspace_files_tool(workspace, &arguments),
            "write_file" => write_workspace_file_tool(workspace, &arguments),
            "create_directory" => create_workspace_directory_tool(workspace, &arguments),
            "delete_path" => delete_workspace_path_tool(workspace, &arguments),
            "move_path" => move_workspace_path_tool(workspace, &arguments),
            "apply_patch" => apply_patch_tool(workspace, &arguments),
            "run_command" => run_workspace_command_tool(workspace, &arguments, stream_sink),
            _ => Err(format!("Unknown tool: {}", name)),
        }
    };

    json!({
        "role": "tool",
        "tool_call_id": tool_call_id,
        "content": content.unwrap_or_else(|error| format!("Tool {} failed: {}", name, error)),
    })
}

fn execute_codegraph_explore_tool(workspace: &Path, arguments: &Value) -> Result<String, String> {
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
                "Unsupported CodeGraph command `{}`. Allowed commands: {}.",
                command,
                CODEGRAPH_READ_COMMANDS
                    .iter()
                    .chain(CODEGRAPH_WRITE_COMMANDS.iter())
                    .copied()
                    .collect::<Vec<_>>()
                    .join(", ")
            ))
        }
    }

    Ok((args, requires_index))
}

fn execute_codegraph_command_tool(workspace: &Path, arguments: &Value) -> Result<String, String> {
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
    let stdout = strip_ansi_escape_sequences(&String::from_utf8_lossy(&output.stdout));
    let stderr = strip_ansi_escape_sequences(&String::from_utf8_lossy(&output.stderr));
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

    Ok(truncate_text(
        format!("CodeGraph {} result:\n{}", command, detail),
        20_000,
    ))
}

fn tool_arg_string<'a>(arguments: &'a Value, key: &str) -> &'a str {
    arguments
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("")
}

fn tool_arg_bool(arguments: &Value, key: &str, default: bool) -> bool {
    arguments
        .get(key)
        .and_then(Value::as_bool)
        .unwrap_or(default)
}

fn tool_arg_string_array(arguments: &Value, key: &str) -> Vec<String> {
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

fn tool_arg_string_map(arguments: &Value, key: &str) -> Vec<(String, String)> {
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

fn tool_arg_usize(arguments: &Value, key: &str, default: usize, min: usize, max: usize) -> usize {
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
        truncate_text(numbered, 18_000)
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
        truncate_text(lines.join("\n"), 18_000)
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
            file_handle
                .write_all(content.as_bytes())
                .map_err(|error| format!("Failed to append {}: {}", file, error))?;
        }
        other => return Err(format!("Unsupported write_file mode: {}", other)),
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

fn apply_patch_tool(workspace: &Path, arguments: &Value) -> Result<String, String> {
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
            truncate_text(output, 8_000)
        }
    ))
}

fn spawn_command_output_reader<R: Read + Send + 'static>(
    mut reader: R,
    stream: CommandOutputStream,
    sender: Sender<CommandOutputEvent>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let mut buffer = [0_u8; COMMAND_STREAM_CHUNK_SIZE];

        loop {
            match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(read_len) => {
                    let chunk = String::from_utf8_lossy(&buffer[..read_len]).to_string();

                    if sender
                        .send(CommandOutputEvent::Chunk(stream, chunk))
                        .is_err()
                    {
                        return;
                    }
                }
                Err(error) => {
                    let _ = sender.send(CommandOutputEvent::ReadError(stream, error.to_string()));
                    break;
                }
            }
        }

        let _ = sender.send(CommandOutputEvent::Done(stream));
    })
}

fn emit_command_output_chunk(
    stream_sink: &mut Option<&mut ToolStreamSink<'_>>,
    stream: CommandOutputStream,
    chunk: &str,
) {
    let Some(sink) = stream_sink.as_deref_mut() else {
        return;
    };

    if chunk.is_empty() {
        return;
    }

    sink(trace_step_with_detail(
        "tool",
        chunk.to_string(),
        stream.label().to_string(),
    ));
}

fn handle_command_output_event(
    event: CommandOutputEvent,
    stdout: &mut String,
    stderr: &mut String,
    stdout_done: &mut bool,
    stderr_done: &mut bool,
    stream_sink: &mut Option<&mut ToolStreamSink<'_>>,
) {
    match event {
        CommandOutputEvent::Chunk(CommandOutputStream::Stdout, chunk) => {
            stdout.push_str(&chunk);
            emit_command_output_chunk(stream_sink, CommandOutputStream::Stdout, &chunk);
        }
        CommandOutputEvent::Chunk(CommandOutputStream::Stderr, chunk) => {
            stderr.push_str(&chunk);
            emit_command_output_chunk(stream_sink, CommandOutputStream::Stderr, &chunk);
        }
        CommandOutputEvent::ReadError(CommandOutputStream::Stdout, error) => {
            *stdout_done = true;
            stderr.push_str(&format!("\n[stdout read error] {}\n", error));
        }
        CommandOutputEvent::ReadError(CommandOutputStream::Stderr, error) => {
            *stderr_done = true;
            stderr.push_str(&format!("\n[stderr read error] {}\n", error));
        }
        CommandOutputEvent::Done(CommandOutputStream::Stdout) => {
            *stdout_done = true;
        }
        CommandOutputEvent::Done(CommandOutputStream::Stderr) => {
            *stderr_done = true;
        }
    }
}

fn drain_command_output_events(
    receiver: &mpsc::Receiver<CommandOutputEvent>,
    stdout: &mut String,
    stderr: &mut String,
    stdout_done: &mut bool,
    stderr_done: &mut bool,
    stream_sink: &mut Option<&mut ToolStreamSink<'_>>,
) {
    while let Ok(event) = receiver.try_recv() {
        handle_command_output_event(event, stdout, stderr, stdout_done, stderr_done, stream_sink);
    }
}

fn run_workspace_command_tool(
    workspace: &Path,
    arguments: &Value,
    mut stream_sink: Option<&mut ToolStreamSink<'_>>,
) -> Result<String, String> {
    let command = tool_arg_string(arguments, "command");
    let args = tool_arg_string_array(arguments, "args");
    let env_overrides = tool_arg_string_map(arguments, "env");
    let timeout_ms = tool_arg_usize(arguments, "timeoutMs", 30_000, 1_000, 120_000) as u64;
    let cwd_arg = tool_arg_string(arguments, "cwd");
    let cwd = if cwd_arg.is_empty() {
        workspace.to_path_buf()
    } else {
        resolve_workspace_relative_path(workspace, cwd_arg)?
    };

    if command.is_empty() {
        return Err("run_command requires a command.".to_string());
    }

    if command.contains('/') || command.contains('\\') {
        return Err("run_command command must be an executable name, not a path.".to_string());
    }

    if !cwd.is_dir() {
        return Err("run_command cwd must be a directory.".to_string());
    }

    let metadata = format!(
        "workspace={}\ncwd={}\nenv_overrides={}\n",
        workspace.display(),
        cwd.display(),
        env_overrides.len()
    );
    let mut command_builder = Command::new(command);
    command_builder
        .current_dir(&cwd)
        .args(&args)
        .envs(env_overrides.iter().map(|(name, value)| (name, value)))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = command_builder
        .spawn()
        .map_err(|error| format!("Failed to start command: {}", error))?;

    let stdout_pipe = child
        .stdout
        .take()
        .ok_or_else(|| "Failed to capture command stdout.".to_string())?;
    let stderr_pipe = child
        .stderr
        .take()
        .ok_or_else(|| "Failed to capture command stderr.".to_string())?;
    let (sender, receiver) = mpsc::channel();
    let stdout_reader =
        spawn_command_output_reader(stdout_pipe, CommandOutputStream::Stdout, sender.clone());
    let stderr_reader =
        spawn_command_output_reader(stderr_pipe, CommandOutputStream::Stderr, sender);
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    let mut stdout = String::new();
    let mut stderr = String::new();
    let mut stdout_done = false;
    let mut stderr_done = false;
    let mut status = None;

    loop {
        drain_command_output_events(
            &receiver,
            &mut stdout,
            &mut stderr,
            &mut stdout_done,
            &mut stderr_done,
            &mut stream_sink,
        );

        if status.is_none() {
            status = child
                .try_wait()
                .map_err(|error| format!("Failed to poll command: {}", error))?;
        }

        if status.is_some() && stdout_done && stderr_done {
            break;
        }

        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child
                .wait()
                .map_err(|error| format!("Failed to wait for timed-out command: {}", error))?;

            while !(stdout_done && stderr_done) {
                match receiver.recv_timeout(Duration::from_millis(100)) {
                    Ok(event) => handle_command_output_event(
                        event,
                        &mut stdout,
                        &mut stderr,
                        &mut stdout_done,
                        &mut stderr_done,
                        &mut stream_sink,
                    ),
                    Err(RecvTimeoutError::Timeout) => break,
                    Err(RecvTimeoutError::Disconnected) => break,
                }
            }

            let _ = stdout_reader.join();
            let _ = stderr_reader.join();
            let combined = format!(
                "{}Command timed out after {} ms.\nstdout:\n{}\nstderr:\n{}",
                metadata,
                timeout_ms,
                stdout.trim(),
                stderr.trim()
            );
            return Err(combined);
        }

        match receiver.recv_timeout(Duration::from_millis(50)) {
            Ok(event) => handle_command_output_event(
                event,
                &mut stdout,
                &mut stderr,
                &mut stdout_done,
                &mut stderr_done,
                &mut stream_sink,
            ),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                if status.is_some() {
                    break;
                }
            }
        }
    }

    let _ = stdout_reader.join();
    let _ = stderr_reader.join();
    drain_command_output_events(
        &receiver,
        &mut stdout,
        &mut stderr,
        &mut stdout_done,
        &mut stderr_done,
        &mut stream_sink,
    );

    let status = status.ok_or_else(|| "Command ended without an exit status.".to_string())?;
    let code = status
        .code()
        .map(|value| value.to_string())
        .unwrap_or_else(|| "terminated".to_string());
    let combined = format!(
        "{}exit_code={}\nstdout:\n{}\nstderr:\n{}",
        metadata,
        code,
        stdout.trim(),
        stderr.trim()
    );

    if !status.success() {
        return Err(combined);
    }

    Ok(combined)
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

fn truncate_text(text: String, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text;
    }

    let mut truncated: String = text.chars().take(max_chars).collect();
    truncated.push_str("\n\n[Content truncated]");
    truncated
}

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

pub(crate) fn strip_ansi_escape_sequences(text: &str) -> String {
    let mut cleaned = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '\x1b' && matches!(chars.peek(), Some('[')) {
            chars.next();
            for next in chars.by_ref() {
                if ('@'..='~').contains(&next) {
                    break;
                }
            }
            continue;
        }

        cleaned.push(ch);
    }

    cleaned
}

fn run_codegraph_status(workspace: &Path) -> Result<String, String> {
    let output = run_codegraph_command(workspace, &["status"])?;
    let stdout = strip_ansi_escape_sequences(&String::from_utf8_lossy(&output.stdout));
    let stderr = strip_ansi_escape_sequences(&String::from_utf8_lossy(&output.stderr));

    if !output.status.success() || stdout.trim().is_empty() {
        return Err(if stderr.trim().is_empty() {
            "CodeGraph status returned no usable result.".to_string()
        } else {
            format!("CodeGraph status failed: {}", stderr.trim())
        });
    }

    Ok(stdout.trim().to_string())
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

    Ok(truncate_text(
        format_codegraph_explore_output(&stdout, status_output.as_deref()),
        18_000,
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
                truncate_text(content, 4_000)
            ));
        }
    }

    Ok(truncate_text(sections.join("\n"), 18_000))
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

fn validate_relative_file(file: &str) -> Result<String, String> {
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
        .map_err(|error| format!("Failed to run git apply: {}", error))?;
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    if !output.status.success() {
        return Err(if stderr.trim().is_empty() {
            format!("git apply failed: {}", stdout)
        } else {
            format!("git apply failed: {}", stderr)
        });
    }

    Ok((stdout, stderr))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_supported_codegraph_read_command_arguments() {
        let (query, query_requires_index) = build_codegraph_command_args(&json!({
            "command": "query",
            "query": "ChatGroupPage",
            "limit": 12,
            "kind": "function",
            "json": true
        }))
        .unwrap();
        assert_eq!(
            query,
            vec![
                "query",
                "--limit",
                "12",
                "--kind",
                "function",
                "--json",
                "ChatGroupPage"
            ]
        );
        assert!(query_requires_index);

        let (node, _) = build_codegraph_command_args(&json!({
            "command": "node",
            "file": "src/main.rs",
            "offset": 20,
            "limit": 40,
            "symbolsOnly": true
        }))
        .unwrap();
        assert_eq!(
            node,
            vec![
                "node",
                "--file",
                "src/main.rs",
                "--offset",
                "20",
                "--limit",
                "40",
                "--symbols-only"
            ]
        );
    }

    #[test]
    fn validates_codegraph_command_arguments_and_index_requirements() {
        assert!(build_codegraph_command_args(&json!({ "command": "query" })).is_err());
        assert!(build_codegraph_command_args(&json!({ "command": "node" })).is_err());
        assert!(build_codegraph_command_args(&json!({ "command": "uninit" })).is_err());

        for command in CODEGRAPH_WRITE_COMMANDS {
            let (args, requires_index) =
                build_codegraph_command_args(&json!({ "command": command })).unwrap();
            assert_eq!(*command == "sync", requires_index);
            assert_eq!(
                matches!(*command, "index" | "sync"),
                args.contains(&"--quiet".to_string())
            );
        }
    }

    #[test]
    fn apply_patch_trace_keeps_details_beyond_the_default_tool_limit() {
        let patch_text = format!(
            "diff --git a/src/example.cpp b/src/example.cpp\n--- a/src/example.cpp\n+++ b/src/example.cpp\n@@ -1 +1 @@\n-{}\n+{}\n",
            "a".repeat(TOOL_TRACE_DETAIL_LIMIT),
            "b".repeat(TOOL_TRACE_DETAIL_LIMIT)
        );
        let tool_call = json!({
            "function": {
                "name": "apply_patch",
                "arguments": serde_json::to_string(&json!({ "patchText": patch_text })).unwrap()
            }
        });

        let step = tool_call_trace_step(&tool_call);
        let detail = step.detail.unwrap();

        assert!(detail.contains(&"b".repeat(TOOL_TRACE_DETAIL_LIMIT)));
        assert!(!detail.contains("[Content truncated]"));
    }

    #[test]
    fn parses_smart_quoted_tool_argument_keys_without_changing_patch_content() {
        let function = json!({
            "arguments": "{\u{201c}patchText\u{201d}:\"const label = \u{201c}保留正文引号\u{201d};\"}"
        });

        let arguments = parsed_tool_arguments(&function);

        assert_eq!(
            arguments["patchText"],
            json!("const label = \u{201c}保留正文引号\u{201d};")
        );
    }

    #[test]
    fn normalizes_relaxed_unified_diff_context_and_hunk_counts() {
        let malformed = concat!(
            "diff --git a/src/example.cpp b/src/example.cpp\n",
            "\u{2014} a/src/example.cpp\n",
            "+++ b/src/example.cpp\n",
            "@@ -20,7 +20,9 @@ public:\n",
            "\tfirst_context();\n",
            "-\told_call();\n",
            "+\tnew_call();\n",
            "\tlast_context();\n",
        );

        let normalized = normalize_relaxed_unified_diff(malformed).unwrap();

        assert_eq!(
            normalized,
            concat!(
                "diff --git a/src/example.cpp b/src/example.cpp\n",
                "--- a/src/example.cpp\n",
                "+++ b/src/example.cpp\n",
                "@@ -20,3 +20,3 @@ public:\n",
                " \tfirst_context();\n",
                "-\told_call();\n",
                "+\tnew_call();\n",
                " \tlast_context();\n",
            )
        );
    }

    #[test]
    fn normalizes_unicode_dash_in_the_old_file_header() {
        let malformed = concat!(
            "diff --git a/src/example.cpp b/src/example.cpp\n",
            "\u{2014} a/src/example.cpp\n",
            "+++ b/src/example.cpp\n",
            "@@ -1 +1 @@\n",
            "-old_call();\n",
            "+new_call();\n",
        );

        let normalized = normalize_relaxed_unified_diff(malformed).unwrap();

        assert_eq!(
            normalized,
            concat!(
                "diff --git a/src/example.cpp b/src/example.cpp\n",
                "--- a/src/example.cpp\n",
                "+++ b/src/example.cpp\n",
                "@@ -1,1 +1,1 @@\n",
                "-old_call();\n",
                "+new_call();\n",
            )
        );
    }

    #[test]
    fn leaves_valid_unified_diff_unchanged() {
        let valid = concat!(
            "diff --git a/src/example.cpp b/src/example.cpp\n",
            "--- a/src/example.cpp\n",
            "+++ b/src/example.cpp\n",
            "@@ -20,3 +20,3 @@ public:\n",
            " \tfirst_context();\n",
            "-\told_call();\n",
            "+\tnew_call();\n",
            " \tlast_context();\n",
        );

        assert_eq!(normalize_relaxed_unified_diff(valid).unwrap(), valid);
    }

    #[test]
    fn apply_patch_repairs_model_generated_diff_before_applying() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let workspace = std::env::temp_dir().join(format!("matrix-patch-repair-test-{}", stamp));
        let source_dir = workspace.join("src");
        fs::create_dir_all(&source_dir).unwrap();
        fs::write(
            source_dir.join("example.cpp"),
            "\tfirst_context();\n\told_call();\n\tlast_context();\n",
        )
        .unwrap();
        let workspace = fs::canonicalize(&workspace).unwrap();
        let malformed = concat!(
            "diff --git a/src/example.cpp b/src/example.cpp\n",
            "\u{2014} a/src/example.cpp\n",
            "+++ b/src/example.cpp\n",
            "@@ -1,7 +1,9 @@\n",
            "\tfirst_context();\n",
            "-\told_call();\n",
            "+\tnew_call();\n",
            "\tlast_context();\n",
        );

        let result = apply_patch_tool(
            &workspace,
            &json!({
                "patchText": malformed
            }),
        )
        .unwrap();

        assert!(result.contains("Patch format was normalized"));
        assert_eq!(
            fs::read_to_string(source_dir.join("example.cpp"))
                .unwrap()
                .replace("\r\n", "\n"),
            "\tfirst_context();\n\tnew_call();\n\tlast_context();\n"
        );

        let _ = fs::remove_dir_all(&workspace);
    }

    #[test]
    fn apply_patch_preserves_a_valid_trailing_newline() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let workspace = std::env::temp_dir().join(format!("matrix-patch-newline-test-{}", stamp));
        fs::create_dir_all(workspace.join("src")).unwrap();
        fs::write(workspace.join("src/example.cpp"), "old_call();\n").unwrap();
        let workspace = fs::canonicalize(&workspace).unwrap();
        let valid = concat!(
            "diff --git a/src/example.cpp b/src/example.cpp\n",
            "--- a/src/example.cpp\n",
            "+++ b/src/example.cpp\n",
            "@@ -1 +1 @@\n",
            "-old_call();\n",
            "+new_call();\n",
        );

        let result = apply_patch_tool(
            &workspace,
            &json!({
                "patchText": valid
            }),
        )
        .unwrap();

        assert!(!result.contains("Patch format was normalized"));
        assert_eq!(
            fs::read_to_string(workspace.join("src/example.cpp"))
                .unwrap()
                .replace("\r\n", "\n"),
            "new_call();\n"
        );

        let _ = fs::remove_dir_all(&workspace);
    }

    #[test]
    fn temporary_patch_files_are_unique_and_preserve_bytes() {
        let patch = "diff --git a/a b/a\n";
        let first = write_temp_patch(patch).unwrap();
        let second = write_temp_patch(patch).unwrap();

        assert_ne!(first, second);
        assert_eq!(fs::read(&first).unwrap(), patch.as_bytes());
        assert_eq!(fs::read(&second).unwrap(), patch.as_bytes());

        let _ = fs::remove_file(first);
        let _ = fs::remove_file(second);
    }

    #[test]
    fn apply_patch_does_not_normalize_content_conflicts() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let workspace = std::env::temp_dir().join(format!("matrix-patch-conflict-test-{}", stamp));
        fs::create_dir_all(workspace.join("src")).unwrap();
        fs::write(workspace.join("src/example.cpp"), "actual();\n").unwrap();
        let workspace = fs::canonicalize(&workspace).unwrap();
        let valid_but_conflicting = concat!(
            "diff --git a/src/example.cpp b/src/example.cpp\n",
            "--- a/src/example.cpp\n",
            "+++ b/src/example.cpp\n",
            "@@ -1 +1 @@\n",
            "-expected();\n",
            "+changed();\n",
        );

        let error = apply_patch_tool(
            &workspace,
            &json!({
                "patchText": valid_but_conflicting
            }),
        )
        .unwrap_err();

        assert!(error.contains("patch does not apply"));
        assert!(!error.contains("normalized"));
        assert_eq!(
            fs::read_to_string(workspace.join("src/example.cpp")).unwrap(),
            "actual();\n"
        );

        let _ = fs::remove_dir_all(&workspace);
    }

    #[test]
    fn run_command_streams_output_before_process_exit() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let workspace = std::env::temp_dir().join(format!("matrix-command-stream-test-{}", stamp));
        fs::create_dir_all(&workspace).unwrap();
        let workspace = fs::canonicalize(&workspace).unwrap();

        #[cfg(windows)]
        let (command, args) = (
            "cmd",
            vec![
                "/C",
                "echo first & powershell -NoProfile -Command Start-Sleep -Milliseconds 600",
            ],
        );
        #[cfg(not(windows))]
        let (command, args) = ("sh", vec!["-c", "printf 'first\\n'; sleep 0.6"]);

        let mut first_chunk_at: Option<Instant> = None;
        let mut chunks = Vec::new();
        let mut sink = |step: ChatTraceStep| {
            if first_chunk_at.is_none() {
                first_chunk_at = Some(Instant::now());
            }

            chunks.push((step.detail.unwrap_or_default(), step.text));
        };

        let result = run_workspace_command_tool(
            &workspace,
            &json!({
                "command": command,
                "args": args,
                "timeoutMs": 5_000
            }),
            Some(&mut sink),
        )
        .unwrap();
        let returned_at = Instant::now();

        assert!(result.contains("stdout:"));
        assert!(result.contains("first"));
        assert!(chunks
            .iter()
            .any(|(stream, text)| stream == "stdout" && text.contains("first")));
        assert!(
            returned_at.duration_since(first_chunk_at.expect("expected a streamed chunk"))
                >= Duration::from_millis(250)
        );

        let _ = fs::remove_dir_all(&workspace);
    }

    #[test]
    fn run_command_reports_cwd_and_applies_env_overrides() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let workspace = std::env::temp_dir().join(format!("matrix-command-env-test-{}", stamp));
        let subdir = workspace.join("child");
        fs::create_dir_all(&subdir).unwrap();
        let workspace = fs::canonicalize(&workspace).unwrap();
        let subdir = fs::canonicalize(&subdir).unwrap();

        #[cfg(windows)]
        let (command, args) = ("cmd", vec!["/C", "cd && echo %MOP_TEST_ENV%"]);
        #[cfg(not(windows))]
        let (command, args) = ("sh", vec!["-c", "pwd; printf '\\n%s\\n' \"$MOP_TEST_ENV\""]);

        let result = run_workspace_command_tool(
            &workspace,
            &json!({
                "command": command,
                "args": args,
                "cwd": "child",
                "env": {
                    "MOP_TEST_ENV": "hello-from-env"
                },
                "timeoutMs": 5_000
            }),
            None,
        )
        .unwrap();

        assert!(result.contains(&format!("cwd={}", subdir.display())));
        assert!(result.contains("env_overrides=1"));
        assert!(result.contains("hello-from-env"));

        let _ = fs::remove_dir_all(&workspace);
    }
}
