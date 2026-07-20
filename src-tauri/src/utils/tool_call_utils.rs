use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ToolCallExecutionPlan {
    pub(crate) executable_count: usize,
    pub(crate) truncated: bool,
}

pub(crate) struct ToolCallUtils;

impl ToolCallUtils {
    pub(crate) fn can_open_next_autonomous_round(
        current_turn_index: usize,
        max_turns: usize,
    ) -> bool {
        current_turn_index.saturating_add(2) < max_turns
    }

    pub(crate) fn checkpoint_budget_remaining(
        tool_calls_since_checkpoint: usize,
        checkpoint_interval: usize,
    ) -> usize {
        if checkpoint_interval == 0 {
            return usize::MAX;
        }

        let remainder = tool_calls_since_checkpoint % checkpoint_interval;
        if remainder == 0 {
            checkpoint_interval
        } else {
            checkpoint_interval - remainder
        }
    }

    pub(crate) fn plan_execution(
        requested_count: usize,
        tool_calls_since_checkpoint: usize,
        checkpoint_interval: usize,
        max_tool_calls_per_turn: usize,
        remaining_total_budget: usize,
    ) -> ToolCallExecutionPlan {
        if requested_count == 0 {
            return ToolCallExecutionPlan {
                executable_count: 0,
                truncated: false,
            };
        }

        let executable_count = requested_count
            .min(Self::checkpoint_budget_remaining(
                tool_calls_since_checkpoint,
                checkpoint_interval,
            ))
            .min(max_tool_calls_per_turn)
            .min(remaining_total_budget);

        ToolCallExecutionPlan {
            executable_count,
            truncated: executable_count < requested_count,
        }
    }

    pub(crate) fn schedule_checkpoint(
        tool_calls_since_checkpoint: &mut usize,
        tool_checkpoint_pending: &mut bool,
        tool_call_count: usize,
        checkpoint_interval: usize,
    ) {
        if tool_call_count == 0 || checkpoint_interval == 0 {
            return;
        }

        *tool_calls_since_checkpoint += tool_call_count;
        if *tool_calls_since_checkpoint >= checkpoint_interval {
            *tool_calls_since_checkpoint %= checkpoint_interval;
            *tool_checkpoint_pending = true;
        }
    }

    pub(crate) fn trim_chat_tool_calls(message: &Value, executable_count: usize) -> Value {
        let Some(tool_calls) = message.get("tool_calls").and_then(Value::as_array) else {
            return message.clone();
        };

        if executable_count == 0 {
            let mut trimmed_message = message.clone();
            if let Some(object) = trimmed_message.as_object_mut() {
                object.remove("tool_calls");
            }
            return trimmed_message;
        }

        if executable_count >= tool_calls.len() {
            return message.clone();
        }

        let mut trimmed_message = message.clone();
        trimmed_message["tool_calls"] =
            Value::Array(tool_calls.iter().take(executable_count).cloned().collect());
        trimmed_message
    }

    pub(crate) fn truncate_tool_calls(tool_calls: &[Value], executable_count: usize) -> Vec<Value> {
        if executable_count >= tool_calls.len() {
            return tool_calls.to_vec();
        }

        tool_calls.iter().take(executable_count).cloned().collect()
    }
}

/// Watches consecutive read-only exploration tool calls. When the model keeps
/// reading without ever editing, the chat loop injects the returned nudge so
/// the agent stops stalling and applies the change (or explains the blocker)
/// instead of looping on read_file forever.
#[derive(Debug, Default)]
pub(crate) struct NoProgressGuard {
    read_streak: usize,
    edits_made: usize,
}

impl NoProgressGuard {
    const READ_ONLY_TOOLS: &'static [&'static str] = &[
        "read_file",
        "list_files",
        "search_files",
        "glob_files",
        "codegraph_explore",
        "web_search",
    ];
    const EDIT_TOOLS: &'static [&'static str] = &[
        "write_file",
        "apply_patch",
        "create_directory",
        "delete_path",
        "move_path",
    ];

    /// Tools removed from the offered schema while `read_tools_blocked` is
    /// active. codegraph_command is included even though it has write
    /// subcommands: during a forced-edit turn the index commands can wait.
    pub(crate) const BLOCKABLE_READ_TOOLS: &'static [&'static str] = &[
        "read_file",
        "list_files",
        "search_files",
        "glob_files",
        "codegraph_explore",
        "codegraph_command",
        "web_search",
    ];

    fn escalate_threshold(&self) -> usize {
        if self.edits_made == 0 { 8 } else { 12 }
    }

    /// True once the read-only streak hits the escalation threshold: the chat
    /// loop should drop BLOCKABLE_READ_TOOLS from the offered tools until an
    /// edit or action breaks the streak. Instructions alone can be argued
    /// around; a missing tool cannot be called.
    pub(crate) fn read_tools_blocked(&self) -> bool {
        self.read_streak >= self.escalate_threshold()
    }

    fn tool_arguments(tool_call: &Value) -> Option<Value> {
        tool_call
            .get("function")
            .and_then(|function| function.get("arguments"))
            .and_then(Value::as_str)
            .and_then(|arguments| serde_json::from_str::<Value>(arguments).ok())
    }

    /// codegraph_command is read-only exploration unless the subcommand
    /// mutates the index (init/index/sync/unlock). run_command counts as
    /// exploration when the command itself only reads (Get-Content,
    /// Select-String, findstr, cat, type, rg, git diff/log/show, ...):
    /// blocking read tools just pushes a stalling model to read via shell.
    fn is_read_only_call(name: &str, tool_call: &Value) -> bool {
        if Self::READ_ONLY_TOOLS.contains(&name) {
            return true;
        }

        if name == "codegraph_command" {
            let command = Self::tool_arguments(tool_call).and_then(|arguments| {
                arguments
                    .get("command")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            });

            return match command.as_deref() {
                Some(command) => crate::tools::schema::CODEGRAPH_READ_COMMANDS.contains(&command),
                // Unreadable arguments: count as exploration so a malformed
                // call cannot dodge the streak.
                None => true,
            };
        }

        if name == "run_command" {
            let Some(arguments) = Self::tool_arguments(tool_call) else {
                // No readable arguments: nothing to classify, treat as an action.
                return false;
            };
            let command = arguments
                .get("command")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let args = arguments
                .get("args")
                .and_then(Value::as_array)
                .map(|args| {
                    args.iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join(" ")
                })
                .unwrap_or_default();

            return Self::shell_command_is_read_like(command, &args);
        }

        false
    }

    fn shell_command_is_read_like(command: &str, args: &str) -> bool {
        const READ_EXECUTABLES: &[&str] = &[
            "cat", "type", "more", "less", "head", "tail", "rg", "grep", "egrep", "fgrep",
            "findstr", "ls", "dir", "get-content", "gc", "select-string", "sls", "get-childitem",
            "gci",
        ];
        const SHELLS: &[&str] = &["cmd", "powershell", "pwsh", "bash", "sh", "zsh"];
        const READ_TOKENS: &[&str] = &[
            "get-content",
            "select-string",
            "get-childitem",
            "findstr",
            "cat",
            "type",
            "rg",
            "grep",
            "more",
            "less",
            "head",
            "tail",
            "ls",
            "dir",
        ];
        const GIT_READ_SUBCOMMANDS: &[&str] = &["diff", "log", "show", "blame", "grep"];

        let basename = command
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or(command)
            .trim()
            .trim_end_matches(".exe")
            .to_ascii_lowercase();

        if READ_EXECUTABLES.contains(&basename.as_str()) {
            return true;
        }

        if basename == "git" {
            let subcommand = args.split_whitespace().next().unwrap_or_default();
            return GIT_READ_SUBCOMMANDS.contains(&subcommand);
        }

        if SHELLS.contains(&basename.as_str()) {
            let haystack = args.to_ascii_lowercase();
            return READ_TOKENS
                .iter()
                .any(|token| contains_shell_word(&haystack, token));
        }

        false
    }

    /// Returns a nudge message when the read-only streak crosses a threshold.
    /// Thresholds are looser once at least one edit landed, because reading
    /// back changed files is legitimate verification.
    pub(crate) fn record_tool_call(&mut self, tool_call: &Value) -> Option<String> {
        let name = tool_call
            .get("function")
            .and_then(|function| function.get("name"))
            .and_then(Value::as_str)
            .unwrap_or_default();

        if Self::EDIT_TOOLS.contains(&name) {
            self.read_streak = 0;
            self.edits_made += 1;
            return None;
        }

        if !Self::is_read_only_call(name, tool_call) {
            // run_command, codegraph init/update, dispatch_tasks, ... are
            // actions: they break a pure reading streak.
            self.read_streak = 0;
            return None;
        }

        self.read_streak += 1;
        let nudge_at = if self.edits_made == 0 { 5 } else { 8 };
        let escalate_at = self.escalate_threshold();

        if self.read_streak == nudge_at {
            return Some(if self.edits_made == 0 {
                format!(
                    "Progress check: {nudge_at} read-only exploration calls in a row and no edits yet. If you already have enough context, stop reading and apply the change now with apply_patch/write_file. If something is blocking you, reply explaining the blocker instead of reading more."
                )
            } else {
                format!(
                    "Progress check: {nudge_at} read-only calls in a row since your last edit. Verify the change (e.g. run_command build/test) and finish, or apply the next edit now — do not keep re-reading files you already saw."
                )
            });
        }

        if self.read_streak >= escalate_at {
            return Some(format!(
                "Stop reading: {} consecutive read-only exploration calls without progress. Read/explore tools (read_file, search_files, glob_files, list_files, codegraph, web_search) are DISABLED until you take action. Apply the edit now with apply_patch/write_file using the context you already have, run a verification command, or write your final reply — the read tools return as soon as you do.",
                self.read_streak
            ));
        }

        None
    }
}

/// Whole-word match for shell command scanning: the token must be bounded by
/// non-alphanumeric characters (or the string edges), so `typecheck`, `mkdir`
/// and `tools` never match `type`, `dir` or `ls`.
fn contains_shell_word(haystack: &str, needle: &str) -> bool {
    let mut start = 0;

    while let Some(offset) = haystack[start..].find(needle) {
        let index = start + offset;
        let before_ok = index == 0
            || !haystack[..index]
                .chars()
                .next_back()
                .is_some_and(char::is_alphanumeric);
        let after_index = index + needle.len();
        let after_ok = after_index >= haystack.len()
            || !haystack[after_index..]
                .chars()
                .next()
                .is_some_and(char::is_alphanumeric);

        if before_ok && after_ok {
            return true;
        }

        start = index + 1;
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn named_tool_call(name: &str) -> Value {
        json!({ "function": { "name": name } })
    }

    fn codegraph_call(command: &str) -> Value {
        json!({
            "function": {
                "name": "codegraph_command",
                "arguments": serde_json::to_string(&json!({ "command": command })).unwrap(),
            }
        })
    }

    fn command_tool_call(command: &str, args: &[&str]) -> Value {
        json!({
            "function": {
                "name": "run_command",
                "arguments": serde_json::to_string(&json!({ "command": command, "args": args })).unwrap(),
            }
        })
    }

    #[test]
    fn no_progress_guard_counts_shell_reads_as_exploration() {
        let mut guard = NoProgressGuard::default();

        // PowerShell/cmd file reads count as exploration, not action.
        for _ in 0..4 {
            assert!(guard
                .record_tool_call(&command_tool_call(
                    "powershell",
                    &["-Command", "Get-Content", "src/lib.rs"]
                ))
                .is_none());
        }
        let nudge = guard
            .record_tool_call(&command_tool_call("cmd", &["/c", "type", "src/lib.rs"]))
            .expect("fifth read-like shell command should nudge");
        assert!(nudge.contains("Progress check"));

        // Builds/tests/typecheck stay actions and break the streak.
        assert!(guard
            .record_tool_call(&command_tool_call("cargo", &["test"]))
            .is_none());
        assert!(guard
            .record_tool_call(&command_tool_call("powershell", &["-Command", "npm run typecheck"]))
            .is_none());

        // git read subcommands count as exploration; git apply is an action.
        assert!(guard
            .record_tool_call(&command_tool_call("git", &["diff"]))
            .is_none());
        assert!(guard
            .record_tool_call(&command_tool_call("git", &["apply", "x.patch"]))
            .is_none());
    }

    #[test]
    fn shell_word_matching_respects_word_boundaries() {
        assert!(contains_shell_word("get-content src/lib.rs", "get-content"));
        assert!(contains_shell_word("/c type file.txt", "type"));
        assert!(!contains_shell_word("npm run typecheck", "type"));
        assert!(!contains_shell_word("mkdir build", "dir"));
        assert!(!contains_shell_word("cargo test", "rg"));
        assert!(!contains_shell_word("list tools", "ls"));
    }

    #[test]
    fn no_progress_guard_nudges_then_escalates_a_pure_read_streak() {
        let mut guard = NoProgressGuard::default();

        for _ in 0..4 {
            assert!(guard.record_tool_call(&named_tool_call("read_file")).is_none());
        }

        let nudge = guard
            .record_tool_call(&named_tool_call("read_file"))
            .expect("fifth consecutive read should nudge");
        assert!(nudge.contains("Progress check"));
        assert!(nudge.contains("apply_patch"));

        assert!(guard.record_tool_call(&named_tool_call("list_files")).is_none());
        assert!(guard.record_tool_call(&named_tool_call("search_files")).is_none());

        let escalation = guard
            .record_tool_call(&named_tool_call("read_file"))
            .expect("eighth consecutive read should escalate");
        assert!(escalation.contains("Stop reading"));
    }

    #[test]
    fn no_progress_guard_resets_on_edits_and_actions() {
        let mut guard = NoProgressGuard::default();

        for _ in 0..4 {
            guard.record_tool_call(&named_tool_call("read_file"));
        }
        assert!(guard.record_tool_call(&named_tool_call("apply_patch")).is_none());

        // After an edit the streak restarts with looser thresholds (8/12).
        for _ in 0..7 {
            assert!(guard.record_tool_call(&named_tool_call("read_file")).is_none());
        }
        let nudge = guard
            .record_tool_call(&named_tool_call("glob_files"))
            .expect("eighth read after an edit should nudge");
        assert!(nudge.contains("since your last edit"));

        // run_command is an action and breaks the streak.
        assert!(guard.record_tool_call(&named_tool_call("run_command")).is_none());
        for _ in 0..7 {
            assert!(guard.record_tool_call(&named_tool_call("read_file")).is_none());
        }
    }

    #[test]
    fn no_progress_guard_counts_codegraph_reads_and_blocks_read_tools() {
        let mut guard = NoProgressGuard::default();
        assert!(!guard.read_tools_blocked());

        // Read subcommands (query/callers/node/files) count toward the streak,
        // so a model cannot dodge the guard by switching from read_file to
        // codegraph_command.
        for _ in 0..4 {
            assert!(guard.record_tool_call(&codegraph_call("query")).is_none());
        }
        assert!(guard.record_tool_call(&codegraph_call("callers")).is_some());
        assert!(guard.record_tool_call(&codegraph_call("node")).is_none());
        assert!(guard.record_tool_call(&codegraph_call("files")).is_none());
        let escalation = guard
            .record_tool_call(&codegraph_call("query"))
            .expect("eighth read should escalate");
        assert!(escalation.contains("DISABLED"));
        assert!(guard.read_tools_blocked());

        // Index-mutating subcommands are actions and break the streak.
        assert!(guard.record_tool_call(&codegraph_call("init")).is_none());
        assert!(!guard.read_tools_blocked());
    }

    #[test]
    fn plans_hard_tool_call_cap_before_checkpoint() {
        let plan = ToolCallUtils::plan_execution(12, 3, 8, 8, 99);

        assert_eq!(plan.executable_count, 5);
        assert!(plan.truncated);
    }

    #[test]
    fn plans_single_tool_turn_when_strict_pacing_is_enabled() {
        let plan = ToolCallUtils::plan_execution(6, 0, 8, 1, 99);

        assert_eq!(plan.executable_count, 1);
        assert!(plan.truncated);
    }

    #[test]
    fn plans_zero_tool_execution_when_total_budget_is_exhausted() {
        let plan = ToolCallUtils::plan_execution(3, 0, 8, 1, 0);

        assert_eq!(plan.executable_count, 0);
        assert!(plan.truncated);
    }

    #[test]
    fn detects_when_another_autonomous_round_is_allowed() {
        assert!(ToolCallUtils::can_open_next_autonomous_round(0, 32));
        assert!(ToolCallUtils::can_open_next_autonomous_round(29, 32));
        assert!(!ToolCallUtils::can_open_next_autonomous_round(30, 32));
        assert!(!ToolCallUtils::can_open_next_autonomous_round(31, 32));
    }

    #[test]
    fn schedules_tool_call_checkpoint_every_eight_calls() {
        let mut tool_calls_since_checkpoint = 0usize;
        let mut tool_checkpoint_pending = false;

        ToolCallUtils::schedule_checkpoint(
            &mut tool_calls_since_checkpoint,
            &mut tool_checkpoint_pending,
            3,
            8,
        );
        assert_eq!(tool_calls_since_checkpoint, 3);
        assert!(!tool_checkpoint_pending);

        ToolCallUtils::schedule_checkpoint(
            &mut tool_calls_since_checkpoint,
            &mut tool_checkpoint_pending,
            5,
            8,
        );
        assert_eq!(tool_calls_since_checkpoint, 0);
        assert!(tool_checkpoint_pending);

        tool_checkpoint_pending = false;
        ToolCallUtils::schedule_checkpoint(
            &mut tool_calls_since_checkpoint,
            &mut tool_checkpoint_pending,
            10,
            8,
        );
        assert_eq!(tool_calls_since_checkpoint, 2);
        assert!(tool_checkpoint_pending);
    }

    #[test]
    fn trims_chat_tool_call_message_to_executed_prefix() {
        let message = json!({
            "role": "assistant",
            "content": "",
            "tool_calls": [
                { "id": "call_1", "type": "function" },
                { "id": "call_2", "type": "function" },
                { "id": "call_3", "type": "function" }
            ]
        });

        let trimmed = ToolCallUtils::trim_chat_tool_calls(&message, 2);

        assert_eq!(trimmed["tool_calls"].as_array().map(Vec::len), Some(2));
        assert_eq!(trimmed["tool_calls"][1]["id"], json!("call_2"));
    }

    #[test]
    fn trims_chat_tool_call_message_to_zero_by_removing_tool_calls() {
        let message = json!({
            "role": "assistant",
            "content": "summary",
            "tool_calls": [
                { "id": "call_1", "type": "function" }
            ]
        });

        let trimmed = ToolCallUtils::trim_chat_tool_calls(&message, 0);

        assert_eq!(trimmed["content"], json!("summary"));
        assert!(trimmed.get("tool_calls").is_none());
    }

    #[test]
    fn truncates_responses_tool_calls_to_executed_prefix() {
        let tool_calls = vec![
            json!({ "call_id": "call_1" }),
            json!({ "call_id": "call_2" }),
            json!({ "call_id": "call_3" }),
        ];

        let truncated = ToolCallUtils::truncate_tool_calls(&tool_calls, 2);

        assert_eq!(truncated.len(), 2);
        assert_eq!(truncated[1]["call_id"], json!("call_2"));
    }
}
