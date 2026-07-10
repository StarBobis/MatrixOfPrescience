use serde_json::{json, Value};
use std::{fs, path::Path};
use tauri::AppHandle;

use crate::{
    append_trace_steps, emit_tool_chunk, emit_trace_step, message_content_text, tools,
    ChatTraceStep,
};

pub(crate) const VALIDATION_REQUIRED_INSTRUCTION: &str = "You changed files in the workspace, so validation must pass before the final visible answer. Call run_command with the most appropriate build, test, type-check, lint, or compile command. Prefer project scripts and manifests already present in the workspace. If validation fails, immediately diagnose the first actionable error, fix its root cause, and rerun validation until it passes. Once a validation command passes, do not rerun the same command merely to be safe; continue any remaining requested work, or answer if the task is done.";
pub(crate) const VALIDATION_FAILURE_RECOVERY_INSTRUCTION: &str = "The previous build, test, type-check, lint, or compile command failed. Do not stop or summarize the failure as the final answer. Inspect the first actionable error and relevant source, fix the root cause with the available workspace tools, then rerun the appropriate validation command. Continue the repair-and-validate loop until validation passes. Once it passes, do not repeat the same validation command merely to be safe; continue remaining work or answer if done. Preserve unrelated user changes and do not hide errors by weakening or skipping validation.";
pub(crate) const VALIDATION_UNAVAILABLE_INSTRUCTION: &str = "No default validation command was detected automatically. Do not provide the final answer yet. Inspect the workspace manifests, build scripts, CI configuration, and documentation to identify the intended build or validation command, then run it. If it fails, fix the root cause and rerun it until it passes. Once validation passes, do not rerun the same command merely to be safe. Only stop for a genuine blocker after exhausting the available workspace tools.";

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

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct ValidationRun {
    ran: bool,
    succeeded: bool,
    successful_command_fingerprints: Vec<String>,
}

impl ValidationRun {
    pub(crate) fn observe_tool_result(&mut self, tool_call: &Value, tool_result: &Value) {
        if is_validation_tool_call(tool_call) {
            let succeeded = tool_result_succeeded(tool_result);
            self.record_result(succeeded);
            if succeeded {
                if let Some(fingerprint) = validation_tool_fingerprint(tool_call) {
                    self.successful_command_fingerprints.push(fingerprint);
                }
            }
        }
    }

    pub(crate) fn record_result(&mut self, succeeded: bool) {
        if !self.ran {
            self.ran = true;
            self.succeeded = true;
        }
        self.succeeded &= succeeded;
    }

    pub(crate) fn ran(&self) -> bool {
        self.ran
    }

    pub(crate) fn succeeded(&self) -> bool {
        self.ran && self.succeeded
    }

    fn successful_command_fingerprints(&self) -> &[String] {
        &self.successful_command_fingerprints
    }
}

#[derive(Debug, Default)]
pub(crate) struct ValidationState {
    required: bool,
    succeeded: bool,
    model_prompted: bool,
    auto_attempted: bool,
    repair_required: bool,
    successful_command_fingerprints: Vec<String>,
}

impl ValidationState {
    pub(crate) fn requires_tool(&self, edit_recovery_required: bool) -> bool {
        self.required && !self.succeeded && !self.repair_required && !edit_recovery_required
    }

    pub(crate) fn requires_repair(&self) -> bool {
        self.repair_required
    }

    pub(crate) fn is_pending(&self) -> bool {
        self.required && !self.succeeded
    }

    pub(crate) fn mark_model_prompted(&mut self) {
        self.model_prompted = true;
    }

    pub(crate) fn mark_successful_edit(&mut self) {
        self.required = true;
        self.succeeded = false;
        self.model_prompted = false;
        self.auto_attempted = false;
        self.repair_required = false;
        self.successful_command_fingerprints.clear();
    }

    pub(crate) fn should_auto_validate(&self, edit_recovery_required: bool) -> bool {
        self.is_pending()
            && self.model_prompted
            && !self.auto_attempted
            && !self.repair_required
            && !edit_recovery_required
    }

    pub(crate) fn can_auto_validate(&self) -> bool {
        self.is_pending() && !self.auto_attempted && !self.repair_required
    }

    pub(crate) fn mark_auto_attempted(&mut self) {
        self.auto_attempted = true;
    }

    pub(crate) fn record_run(&mut self, run: ValidationRun) -> bool {
        if !run.ran() {
            return false;
        }

        if run.succeeded() {
            if self.required {
                self.succeeded = true;
            }
            for fingerprint in run.successful_command_fingerprints() {
                if !self.successful_command_fingerprints.contains(fingerprint) {
                    self.successful_command_fingerprints
                        .push(fingerprint.to_string());
                }
            }
            self.repair_required = false;
            return false;
        }

        self.required = true;
        self.succeeded = false;
        self.repair_required = true;
        true
    }

    pub(crate) fn is_redundant_successful_validation(&self, tool_call: &Value) -> bool {
        self.succeeded
            && validation_tool_fingerprint(tool_call).is_some_and(|fingerprint| {
                self.successful_command_fingerprints.contains(&fingerprint)
            })
    }

    pub(crate) fn redundant_validation_tool_result(&self, tool_call: &Value) -> Option<Value> {
        if !self.is_redundant_successful_validation(tool_call) {
            return None;
        }

        Some(json!({
            "role": "tool",
            "tool_call_id": tool_call.get("id").and_then(Value::as_str).unwrap_or("validation-tool-call"),
            "content": "Validation already passed for this exact command. Do not rerun the same validation just to be safe; continue any remaining requested work, or provide the final answer if the task is complete.",
        }))
    }

    pub(crate) fn mark_validator_discovery_required(&mut self) {
        self.required = true;
        self.succeeded = false;
        self.repair_required = true;
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

fn validation_tool_fingerprint(tool_call: &Value) -> Option<String> {
    if !is_validation_tool_call(tool_call) {
        return None;
    }

    let arguments = tool_call_arguments(tool_call);
    Some(run_command_fingerprint(&arguments))
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

fn run_command_fingerprint(arguments: &Value) -> String {
    let cwd = arguments
        .get("cwd")
        .and_then(Value::as_str)
        .unwrap_or_default();

    format!("{} cwd={}", run_command_text(arguments), cwd).to_ascii_lowercase()
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

pub(crate) fn validation_tool_calls(workspace: &Path) -> Vec<Value> {
    default_validation_commands(workspace)
        .iter()
        .enumerate()
        .map(|(index, command)| validation_tool_call(index, command))
        .collect()
}

pub(crate) fn run_default_validation_commands(
    app: &AppHandle,
    stream_id: Option<&str>,
    workspace: &Path,
    can_write: bool,
    messages: &mut Vec<Value>,
    trace_steps: &mut Vec<ChatTraceStep>,
) -> ValidationRun {
    let tool_calls = validation_tool_calls(workspace);

    if tool_calls.is_empty() {
        return ValidationRun::default();
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
        return ValidationRun::default();
    };

    let mut run = ValidationRun::default();
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
        run.observe_tool_result(&tool_call, &tool_result);
        messages.push(tool_result);
    }

    run
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
        assert!(is_validation_tool_call(&json!({
            "function": {
                "name": "run_command",
                "arguments": "{\"command\":\"powershell\",\"args\":[\"-File\",\"build_debug_x64.ps1\"]}"
            }
        })));
    }

    #[test]
    fn failed_validation_requires_repair_until_a_later_run_passes() {
        let validation_call = json!({
            "function": {
                "name": "run_command",
                "arguments": "{\"command\":\"powershell\",\"args\":[\"-File\",\"build_debug_x64.ps1\"]}"
            }
        });
        let failed_result = json!({
            "role": "tool",
            "content": "Tool run_command failed: exit_code=1\nstderr:\nerror C2838"
        });
        let successful_result = json!({
            "role": "tool",
            "content": "exit_code=0\nstdout:\nBuild succeeded"
        });
        let mut state = ValidationState::default();
        state.mark_successful_edit();

        let mut failed_run = ValidationRun::default();
        failed_run.observe_tool_result(&validation_call, &failed_result);
        assert!(state.record_run(failed_run));
        assert!(state.is_pending());
        assert!(state.requires_repair());
        assert!(!state.requires_tool(false));

        state.mark_successful_edit();
        assert!(!state.requires_repair());
        assert!(state.requires_tool(false));

        let mut successful_run = ValidationRun::default();
        successful_run.observe_tool_result(&validation_call, &successful_result);
        assert!(!state.record_run(successful_run));
        assert!(!state.is_pending());
        assert!(!state.requires_repair());
    }

    #[test]
    fn repeated_successful_validation_command_is_redundant_until_next_edit() {
        let validation_call = json!({
            "id": "validation-1",
            "function": {
                "name": "run_command",
                "arguments": "{\"command\":\"powershell\",\"args\":[\"-File\",\"scripts/build-cmake.ps1\",\"-Configuration\",\"Debug\"]}"
            }
        });
        let different_validation_call = json!({
            "id": "validation-2",
            "function": {
                "name": "run_command",
                "arguments": "{\"command\":\"powershell\",\"args\":[\"-File\",\"scripts/build-cmake.ps1\",\"-Configuration\",\"Release\"]}"
            }
        });
        let successful_result = json!({
            "role": "tool",
            "content": "exit_code=0\nstdout:\nBuild succeeded"
        });
        let mut state = ValidationState::default();
        state.mark_successful_edit();

        let mut successful_run = ValidationRun::default();
        successful_run.observe_tool_result(&validation_call, &successful_result);
        assert!(!state.record_run(successful_run));

        assert!(state.is_redundant_successful_validation(&validation_call));
        assert!(state
            .redundant_validation_tool_result(&validation_call)
            .unwrap()["content"]
            .as_str()
            .unwrap()
            .contains("Validation already passed"));
        assert!(!state.is_redundant_successful_validation(&different_validation_call));

        state.mark_successful_edit();
        assert!(!state.is_redundant_successful_validation(&validation_call));
    }

    #[test]
    fn any_failed_validation_in_a_batch_keeps_repair_required() {
        let mut run = ValidationRun::default();
        run.record_result(true);
        run.record_result(false);

        let mut state = ValidationState::default();
        state.mark_successful_edit();

        assert!(state.record_run(run));
        assert!(state.requires_repair());
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
