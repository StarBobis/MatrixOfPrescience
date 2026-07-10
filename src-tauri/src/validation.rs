use serde_json::{json, Value};
use std::{fs, path::Path};
use tauri::AppHandle;

use crate::{
    append_trace_steps, emit_tool_chunk, emit_trace_step, message_content_text, tools,
    ChatTraceStep,
};

pub(crate) const VALIDATION_REQUIRED_INSTRUCTION: &str = "You just changed files in the workspace. Before writing the final visible answer, call run_command with the most appropriate build, test, type-check, lint, or compile command for the changed project. Prefer project scripts and manifests already present in the workspace, such as npm run build, cargo test, cargo check, pytest, go test, dotnet test, or equivalent. Do not provide a final answer until validation output is available; if validation fails, explain the failure and what remains.";
const VALIDATION_UNAVAILABLE_INSTRUCTION: &str = "No default validation command could be detected automatically for this workspace. Write the final answer now and explicitly say that code was changed but automatic validation could not be run because no known project validator was found.";

fn tool_call_name(tool_call: &Value) -> &str {
    tool_call
        .get("function")
        .and_then(|function| function.get("name"))
        .and_then(Value::as_str)
        .unwrap_or("")
}

fn tool_call_arguments(tool_call: &Value) -> Value {
    let arguments = tool_call
        .get("function")
        .and_then(|function| function.get("arguments"))
        .cloned()
        .unwrap_or_else(|| json!("{}"));

    if let Some(text) = arguments.as_str() {
        serde_json::from_str::<Value>(text).unwrap_or_else(|_| json!({}))
    } else {
        arguments
    }
}

pub(crate) fn tool_result_succeeded(tool_result: &Value) -> bool {
    let content = message_content_text(tool_result);
    !(content.starts_with("Tool ") && content.contains(" failed:"))
}

pub(crate) fn is_successful_edit_tool_call(tool_call: &Value) -> bool {
    match tool_call_name(tool_call) {
        "write_file" | "create_directory" | "delete_path" | "move_path" => true,
        "apply_patch" => !tool_call_arguments(tool_call)
            .get("checkOnly")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        "run_command" => run_command_looks_mutating(&tool_call_arguments(tool_call)),
        _ => false,
    }
}

fn run_command_looks_mutating(arguments: &Value) -> bool {
    let text = run_command_text(arguments);

    [
        "apply",
        "install",
        "set-content",
        "out-file",
        "new-item",
        "remove-item",
        "move-item",
        "copy-item",
        "mkdir",
        "rmdir",
        "rm ",
        "del ",
        "move ",
        "copy ",
        ">",
        ">>",
    ]
    .iter()
    .any(|needle| text.contains(needle))
}

pub(crate) fn is_validation_tool_call(tool_call: &Value) -> bool {
    tool_call_name(tool_call) == "run_command"
        && run_command_looks_like_validation(&tool_call_arguments(tool_call))
}

fn run_command_text(arguments: &Value) -> String {
    let command = arguments
        .get("command")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let args = arguments
        .get("args")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(" ")
        })
        .unwrap_or_default();

    format!("{} {}", command, args).to_ascii_lowercase()
}

fn run_command_looks_like_validation(arguments: &Value) -> bool {
    let text = run_command_text(arguments);

    [
        " build",
        " test",
        " check",
        " lint",
        " typecheck",
        " type-check",
        " tsc",
        "vue-tsc",
        "pytest",
        "cargo test",
        "cargo check",
        "cargo build",
        "go test",
        "dotnet test",
        "mvn test",
        "gradle test",
        "ctest",
        "msbuild",
        "make",
        "ninja",
    ]
    .iter()
    .any(|needle| text.contains(needle))
}

#[derive(Debug, Clone)]
struct ValidationCommand {
    command: String,
    args: Vec<String>,
    cwd: Option<String>,
}

fn package_script_exists(workspace: &Path, script: &str) -> bool {
    let Ok(content) = fs::read_to_string(workspace.join("package.json")) else {
        return false;
    };
    let Ok(parsed) = serde_json::from_str::<Value>(&content) else {
        return false;
    };

    parsed
        .get("scripts")
        .and_then(Value::as_object)
        .and_then(|scripts| scripts.get(script))
        .and_then(Value::as_str)
        .map(|value| !value.trim().is_empty())
        .unwrap_or(false)
}

fn default_validation_commands(workspace: &Path) -> Vec<ValidationCommand> {
    let mut commands = Vec::new();

    for script in ["build", "test", "typecheck", "lint"] {
        if package_script_exists(workspace, script) {
            commands.push(ValidationCommand {
                command: "npm".to_string(),
                args: if script == "test" {
                    vec!["test".to_string()]
                } else {
                    vec!["run".to_string(), script.to_string()]
                },
                cwd: None,
            });
            break;
        }
    }

    if workspace.join("Cargo.toml").is_file() {
        commands.push(ValidationCommand {
            command: "cargo".to_string(),
            args: vec!["test".to_string()],
            cwd: None,
        });
    } else if workspace.join("src-tauri").join("Cargo.toml").is_file() {
        commands.push(ValidationCommand {
            command: "cargo".to_string(),
            args: vec!["test".to_string()],
            cwd: Some("src-tauri".to_string()),
        });
    }

    if workspace.join("pyproject.toml").is_file() || workspace.join("pytest.ini").is_file() {
        commands.push(ValidationCommand {
            command: "python".to_string(),
            args: vec!["-m".to_string(), "pytest".to_string()],
            cwd: None,
        });
    }

    commands
}

fn validation_tool_call(index: usize, command: &ValidationCommand) -> Value {
    let mut arguments = json!({
        "command": command.command,
        "args": command.args,
        "timeoutMs": 120000,
    });

    if let Some(cwd) = command.cwd.as_deref() {
        arguments["cwd"] = json!(cwd);
    }

    json!({
        "id": format!("auto-validation-{}", index),
        "type": "function",
        "function": {
            "name": "run_command",
            "arguments": serde_json::to_string(&arguments).unwrap_or_else(|_| "{}".to_string()),
        }
    })
}

pub(crate) fn run_default_validation_commands(
    app: &AppHandle,
    stream_id: Option<&str>,
    workspace: &Path,
    can_write: bool,
    messages: &mut Vec<Value>,
    trace_steps: &mut Vec<ChatTraceStep>,
) -> bool {
    let tool_calls = default_validation_commands(workspace)
        .iter()
        .enumerate()
        .map(|(index, command)| validation_tool_call(index, command))
        .collect::<Vec<_>>();

    if tool_calls.is_empty() {
        return false;
    }

    messages.push(json!({
        "role": "assistant",
        "content": "",
        "tool_calls": tool_calls,
    }));

    let Some(tool_calls) = messages
        .last()
        .and_then(|message| message.get("tool_calls"))
        .and_then(Value::as_array)
        .cloned()
    else {
        return false;
    };

    for tool_call in tool_calls {
        let call_step = tools::tool_call_trace_step(&tool_call);
        emit_trace_step(app, stream_id, &call_step);
        append_trace_steps(trace_steps, vec![call_step]);

        let mut stream_tool_output = |step: ChatTraceStep| {
            emit_tool_chunk(app, stream_id, &step);
        };
        let tool_result = tools::execute_code_tool_call(
            workspace,
            &tool_call,
            can_write,
            Some(&mut stream_tool_output),
        );
        let result_step = tools::tool_result_trace_step(&tool_call, &tool_result);
        emit_trace_step(app, stream_id, &result_step);
        append_trace_steps(trace_steps, vec![result_step]);
        messages.push(tool_result);
    }

    true
}

pub(crate) fn mark_validation_unavailable(messages: &mut Vec<Value>) {
    messages.push(json!({
        "role": "user",
        "content": VALIDATION_UNAVAILABLE_INSTRUCTION,
    }));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn detects_validation_and_edit_tool_calls() {
        assert!(is_successful_edit_tool_call(&json!({
            "function": {
                "name": "write_file",
                "arguments": "{\"file\":\"src/lib.rs\",\"content\":\"x\"}"
            }
        })));
        assert!(!is_successful_edit_tool_call(&json!({
            "function": {
                "name": "apply_patch",
                "arguments": "{\"patchText\":\"diff --git a/a b/a\",\"checkOnly\":true}"
            }
        })));
        assert!(is_validation_tool_call(&json!({
            "function": {
                "name": "run_command",
                "arguments": "{\"command\":\"npm\",\"args\":[\"run\",\"build\"]}"
            }
        })));
        assert!(!is_validation_tool_call(&json!({
            "function": {
                "name": "run_command",
                "arguments": "{\"command\":\"git\",\"args\":[\"status\"]}"
            }
        })));
    }

    #[test]
    fn default_validation_commands_detect_package_and_tauri_cargo() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let workspace = std::env::temp_dir().join(format!("matrix-validation-test-{}", stamp));
        fs::create_dir_all(workspace.join("src-tauri")).unwrap();
        fs::write(
            workspace.join("package.json"),
            r#"{"scripts":{"build":"vite build"}}"#,
        )
        .unwrap();
        fs::write(
            workspace.join("src-tauri").join("Cargo.toml"),
            "[package]\n",
        )
        .unwrap();

        let commands = default_validation_commands(&workspace);

        assert_eq!(commands.len(), 2);
        assert_eq!(commands[0].command, "npm");
        assert_eq!(commands[0].args, vec!["run", "build"]);
        assert_eq!(commands[1].command, "cargo");
        assert_eq!(commands[1].args, vec!["test"]);
        assert_eq!(commands[1].cwd.as_deref(), Some("src-tauri"));

        let _ = fs::remove_dir_all(workspace);
    }
}
