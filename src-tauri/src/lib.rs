// ── Tauri backend entry point ───────────────────────────────────────────

use base64::{engine::general_purpose, Engine as _};
use futures_util::StreamExt;
use serde_json::json;
use std::{
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tauri::{AppHandle, Manager};

// ── Sub-modules ──────────────────────────────────────────────────────────

mod dsml;
mod tools;
mod utils;
mod validation;

mod types;
mod config;
mod probe;
mod cache;
mod chat;

use dsml::normalize_dsml_tool_calls_in_message;
use tools::{
    code_tools_schema, execute_code_tool_call, orchestration_tools_schema,
    tool_call_trace_step, tool_result_trace_step, validate_workspace,
};
use utils::agent_turn_utils::{AgentReflectionDecision, AgentTurnPhase, AgentTurnState};
use utils::tool_call_utils::{NoProgressGuard, ToolCallUtils};
use validation::{
    ValidationOps, ValidationRun, ValidationState, VALIDATION_FAILURE_RECOVERY_INSTRUCTION,
    VALIDATION_REQUIRED_INSTRUCTION, VALIDATION_UNAVAILABLE_INSTRUCTION,
};

// Re-export shared struct types
pub(crate) use utils::string_utils::StrUtils;
pub(crate) use utils::trace_utils::{ChatCompletionUsage, ChatTraceStep, TraceCtx};

// Re-export items moved to submodules (needed by tests via use super::*)
pub(crate) use types::*;
pub(crate) use config::*;

#[cfg(test)]
use tools::{
    delete_workspace_path_tool, format_codegraph_explore_output, is_codegraph_status_query,
    move_workspace_path_tool, normalize_codegraph_max_files, read_workspace_file_tool,
    resolve_workspace_relative_path, write_workspace_file_tool, DEFAULT_CODEGRAPH_MAX_FILES,
    MAX_CODEGRAPH_MAX_FILES,
};

// ── Shared constants ─────────────────────────────────────────────────────

pub(crate) const MAX_CHAT_COMPLETION_TURNS: usize = 32;
#[cfg(test)]
pub(crate) const MAX_EDIT_RECOVERY_TOOL_ROUNDS: usize = 4;
pub(crate) const CHAT_COMPLETION_STREAM_EVENT: &str = "chat-completion-stream";
pub(crate) const HTTP_RETRY_DELAY: Duration = Duration::from_secs(5);
pub(crate) const RETRY_CANCELLATION_POLL_INTERVAL: Duration = Duration::from_millis(250);
pub(crate) const FINAL_ANSWER_INSTRUCTION: &str =
    "Use the tool results already provided and write the final answer now.";
pub(crate) const CONTINUE_OUTPUT_INSTRUCTION: &str =
    "Continue exactly from where you left off. Do not restart, repeat, or summarize prior text. Finish the same answer.";
pub(crate) const EDIT_FAILURE_RECOVERY_INSTRUCTION: &str = "The previous edit tool call failed. Do not stop or provide a final answer solely because an edit did not apply. Recover using the error and the current workspace state: re-read the target when the context may be stale, then retry with a corrected smaller patch or a different available edit tool. Do not repeat the identical failing call. Continue until the requested change is complete or no available tool can resolve a genuine blocker.";
pub(crate) const FOLLOW_THROUGH_INSTRUCTION: &str = "Your last reply announced more work (\"let me …\", \"让我…\", \"待确认…\") but no tool was called. Follow through now: call the tools needed to finish the work, or — if the task is genuinely complete or blocked — state that plainly without promising further action.";
pub(crate) const MAX_TOOL_ONLY_ROUNDS: usize = 6;
pub(crate) const MAX_UNPRODUCTIVE_TURNS: usize = 3;
pub(crate) const MAX_TOTAL_TOOL_CALLS_PER_AUTONOMOUS_ROUND: usize = 24;
pub(crate) const MAX_DEEPSEEK_TOOL_CALLS_PER_TURN: usize = 1;
pub(crate) const TOOL_CALL_CHECKPOINT_INTERVAL: usize = 8;
pub(crate) const TOOL_CALL_CHECKPOINT_REASONING_EFFORT: &str = "high";
pub(crate) const TOOL_CALL_CHECKPOINT_INSTRUCTION: &str = "Checkpoint: the tool-call budget for this step has been reached. On this turn, do not call any tools. Briefly summarize what you have learned, what remains uncertain, and the single best next step. Keep it concise. After this checkpoint, continue the task on the following turn without waiting for user input if more work is needed.";
pub(crate) const DEEPSEEK_TOOL_ACTION_INSTRUCTION: &str = "This is a tool action turn. If more work is needed, call the single most useful tool now, guided by the latest reflection and tool results. If the task is already complete or no tool is needed, do not call a tool — write the final answer now, starting it with `FINAL:` when you can.";
pub(crate) const CACHE_LOCATION_FILE: &str = "cache-location.json";
pub(crate) const SETTINGS_FILE: &str = "settings.json";
pub(crate) const MEMBER_LIBRARY_FILE: &str = "member-library.json";
pub(crate) const AVATAR_DIR: &str = "avatars";


mod tests {
    
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

    fn spawn_status_then_success_server(
        retry_status: &'static str,
        retry_body: &'static str,
        retry_count: usize,
    ) -> (String, thread::JoinHandle<usize>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("test server should bind");
        let address = listener
            .local_addr()
            .expect("test server should have an address");
        let handle = thread::spawn(move || {
            let mut requests = 0;

            for attempt in 0..=retry_count {
                let (mut stream, _) = listener.accept().expect("test server should accept");
                let mut request = [0_u8; 4096];
                let _ = stream
                    .read(&mut request)
                    .expect("test server should read request");
                requests += 1;

                let (status, body) = if attempt < retry_count {
                    (retry_status, retry_body)
                } else {
                    ("200 OK", r#"{"output_text":"recovered"}"#)
                };
                let response = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                stream
                    .write_all(response.as_bytes())
                    .expect("test server should write response");
            }

            requests
        });

        (format!("http://{address}/v1/responses"), handle)
    }

    fn spawn_overload_then_success_server(
        overload_count: usize,
    ) -> (String, thread::JoinHandle<usize>) {
        spawn_status_then_success_server(
            "503 Service Unavailable",
            r#"{"error":{"message":"system cpu overloaded (current: 93.1%, threshold: 85%)","type":"new_api_error","param":"","code":"system_cpu_overloaded"}}"#,
            overload_count,
        )
    }

    #[test]
    fn normalizes_codegraph_max_files() {
        assert_eq!(
            normalize_codegraph_max_files(None),
            DEFAULT_CODEGRAPH_MAX_FILES
        );
        assert_eq!(normalize_codegraph_max_files(Some(0)), 1);
        assert_eq!(normalize_codegraph_max_files(Some(7)), 7);
        assert_eq!(
            normalize_codegraph_max_files(Some(99)),
            MAX_CODEGRAPH_MAX_FILES
        );
    }

    #[test]
    fn chat_completion_turn_guard_allows_multiple_tool_rounds() {
        assert!(MAX_CHAT_COMPLETION_TURNS >= 16);
    }

    #[test]
    fn http_retry_delay_is_fixed_at_five_seconds() {
        assert_eq!(HTTP_RETRY_DELAY, Duration::from_secs(5));
    }

    #[test]
    fn retries_new_api_resource_overload_until_request_succeeds() {
        let (endpoint, server) = spawn_overload_then_success_server(2);
        let progress = Mutex::new(Vec::new());
        let record_progress = |event| {
            progress
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push(event)
        };
        let response = tauri::async_runtime::block_on(send_http_request_with_retry(
            &reqwest::Client::new(),
            &endpoint,
            "test-key",
            "ChatGPT",
            &json!({ "model": "gpt-5.5", "input": "test" }),
            None,
            Some(&record_progress),
            std::time::Duration::ZERO,
        ))
        .expect("resource overload should be retried");
        let body: Value =
            tauri::async_runtime::block_on(response.json()).expect("response should parse");

        assert_eq!(body["output_text"], "recovered");
        assert_eq!(server.join().expect("test server should finish"), 3);
        assert_eq!(
            progress
                .into_inner()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
            vec![
                HttpRetryProgress::Waiting {
                    attempt: 1,
                    delay: Duration::ZERO,
                    reason: r#"ChatGPT returned HTTP 503 Service Unavailable: {"error":{"message":"system cpu overloaded (current: 93.1%, threshold: 85%)","type":"new_api_error","param":"","code":"system_cpu_overloaded"}}"#.to_string(),
                },
                HttpRetryProgress::Waiting {
                    attempt: 2,
                    delay: Duration::ZERO,
                    reason: r#"ChatGPT returned HTTP 503 Service Unavailable: {"error":{"message":"system cpu overloaded (current: 93.1%, threshold: 85%)","type":"new_api_error","param":"","code":"system_cpu_overloaded"}}"#.to_string(),
                },
                HttpRetryProgress::Recovered { attempts: 2 },
            ]
        );
    }

    #[test]
    fn retries_bad_gateway_until_request_succeeds() {
        let (endpoint, server) =
            spawn_status_then_success_server("502 Bad Gateway", "error code: 502", 1);
        let progress = Mutex::new(Vec::new());
        let record_progress = |event| {
            progress
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push(event)
        };
        let response = tauri::async_runtime::block_on(send_http_request_with_retry(
            &reqwest::Client::new(),
            &endpoint,
            "test-key",
            "ChatGPT",
            &json!({ "model": "gpt-5.5", "input": "test" }),
            None,
            Some(&record_progress),
            std::time::Duration::ZERO,
        ))
        .expect("bad gateway should be retried");
        let body: Value =
            tauri::async_runtime::block_on(response.json()).expect("response should parse");

        assert_eq!(body["output_text"], "recovered");
        assert_eq!(server.join().expect("test server should finish"), 2);
        assert_eq!(
            progress
                .into_inner()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
            vec![
                HttpRetryProgress::Waiting {
                    attempt: 1,
                    delay: Duration::ZERO,
                    reason: "ChatGPT returned HTTP 502 Bad Gateway: error code: 502".to_string(),
                },
                HttpRetryProgress::Recovered { attempts: 1 },
            ]
        );
    }

    #[test]
    fn retries_only_transient_http_failures() {
        assert!(!should_retry_http_failure(
            reqwest::StatusCode::BAD_REQUEST,
            "bad request",
        ));
        assert!(should_retry_http_failure(
            reqwest::StatusCode::TOO_MANY_REQUESTS,
            "rate limited",
        ));
        assert!(should_retry_http_failure(
            reqwest::StatusCode::BAD_GATEWAY,
            "bad gateway",
        ));
        assert!(should_retry_http_failure(
            reqwest::StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
        ));
        assert!(!should_retry_http_failure(reqwest::StatusCode::OK, "{}"));
    }

    #[test]
    fn cancelled_retry_stops_before_sending_another_request() {
        let cancellation = AtomicBool::new(true);
        let error = tauri::async_runtime::block_on(send_http_request_with_retry(
            &reqwest::Client::new(),
            "http://127.0.0.1:1/v1/responses",
            "test-key",
            "ChatGPT",
            &json!({ "model": "gpt-5.5", "input": "test" }),
            Some(&cancellation),
            None,
            Duration::ZERO,
        ))
        .expect_err("cancelled request should stop");

        assert_eq!(error, "Chat completion was cancelled.");
    }

    #[test]
    fn detects_codegraph_status_queries_without_matching_file_names() {
        assert!(is_codegraph_status_query("check CodeGraph status"));
        assert!(is_codegraph_status_query(
            "show index statistics and file count"
        ));
        assert!(!is_codegraph_status_query("open src/i18n/index.ts"));
    }

    #[test]
    fn strips_ansi_escape_sequences_from_status_output() {
        assert_eq!(
            StrUtils::strip_ansi_escape_sequences(
                "\x1b[1mCodeGraph Status\x1b[0m\n\x1b[32m[OK]\x1b[0m"
            ),
            "CodeGraph Status\n[OK]"
        );
    }

    #[test]
    fn formatted_codegraph_output_clarifies_query_scope() {
        let output = format_codegraph_explore_output(
            "Found 44 symbols across 3 files.\n\n**Source Code**",
            Some("Index Statistics:\n  Files:     26"),
        );

        assert!(output.contains("query's returned relevant symbols/files"));
        assert!(output.contains("not the total CodeGraph index file count"));
        assert!(output.contains("Files:     26"));
        assert!(output.contains("CodeGraph explore result:"));
    }

    #[test]
    fn deepseek_reasoning_payload_can_disable_thinking() {
        let mut payload = json!({});

        apply_reasoning_payload(&mut payload, true, Some("off"));

        assert_eq!(payload["thinking"], json!({ "type": "disabled" }));
        assert!(payload.get("reasoning_effort").is_none());
    }

    #[test]
    fn deepseek_reasoning_payload_enables_thinking_with_effort() {
        let mut payload = json!({});

        apply_reasoning_payload(&mut payload, true, Some("high"));

        assert_eq!(payload["thinking"], json!({ "type": "enabled" }));
        assert_eq!(payload["reasoning_effort"], json!("high"));
    }

    #[test]
    fn chat_message_deserializes_reasoning_content_from_camel_case() {
        let message: ChatMessage = serde_json::from_value(json!({
            "role": "assistant",
            "content": "done",
            "reasoningContent": "step one"
        }))
        .expect("camelCase reasoningContent should deserialize");

        assert_eq!(message.reasoning_content.as_deref(), Some("step one"));
    }

    #[test]
    fn chat_message_deserializes_reasoning_content_from_snake_case_alias() {
        let message: ChatMessage = serde_json::from_value(json!({
            "role": "assistant",
            "content": "done",
            "reasoning_content": "step one"
        }))
        .expect("snake_case reasoning_content should deserialize");

        assert_eq!(message.reasoning_content.as_deref(), Some("step one"));
    }

    #[test]
    fn backfills_missing_reasoning_content_on_tool_call_messages() {
        let mut messages = vec![
            json!({ "role": "user", "content": "hi" }),
            json!({
                "role": "assistant",
                "content": "",
                "tool_calls": [
                    { "id": "call_1", "type": "function", "function": { "name": "read_file", "arguments": "{}" } }
                ],
            }),
            json!({
                "role": "assistant",
                "content": "",
                "reasoning_content": "kept",
                "tool_calls": [
                    { "id": "call_2", "type": "function", "function": { "name": "write_file", "arguments": "{}" } }
                ],
            }),
            json!({ "role": "tool", "tool_call_id": "call_1", "content": "ok" }),
        ];

        backfill_deepseek_reasoning(&mut messages);

        // A tool-calling assistant message without reasoning gets a placeholder…
        assert!(messages[1]["reasoning_content"]
            .as_str()
            .is_some_and(|value| !value.is_empty()));
        // …existing reasoning is preserved…
        assert_eq!(messages[2]["reasoning_content"], json!("kept"));
        // …and non-assistant messages stay untouched.
        assert!(messages[0].get("reasoning_content").is_none());
        assert!(messages[3].get("reasoning_content").is_none());
    }

    #[test]
    fn detects_continuation_intent_in_final_replies() {
        // Announced intent at the end counts as unfinished…
        assert!(content_signals_continuation_intent(
            "对，日志确实不可用。但我看了源码后发现了关键差异——让我把完整分析讲完，然后给出可执行的计划。"
        ));
        assert!(content_signals_continuation_intent(
            "Let me quickly check the files."
        ));
        assert!(content_signals_continuation_intent(
            "## 最终结论\n分析如上。\n\n**待确认**：\n1. 添加 slot index 日志\n2. 对比参数"
        ));
        // …plain conclusions and mid-sentence mentions do not.
        assert!(!content_signals_continuation_intent(
            "已完成全部修复，所有测试通过。"
        ));
        assert!(!content_signals_continuation_intent(
            "任务已完成：他让我帮忙的部分也做完了。"
        ));
        assert!(!content_signals_continuation_intent(""));
    }

    #[test]
    fn repair_model_history_keeps_legal_history_untouched() {
        let mut messages = vec![
            json!({ "role": "user", "content": "hi" }),
            json!({
                "role": "assistant",
                "content": "checking",
                "tool_calls": [
                    { "id": "c1", "type": "function", "function": { "name": "read_file", "arguments": "{}" } }
                ],
            }),
            json!({ "role": "tool", "tool_call_id": "c1", "content": "ok" }),
        ];
        let before = messages.clone();

        repair_model_history(&mut messages);

        assert_eq!(messages, before);
    }

    #[test]
    fn repair_model_history_drops_orphan_tool_results() {
        let mut messages = vec![
            json!({ "role": "user", "content": "hi" }),
            json!({ "role": "tool", "tool_call_id": "ghost", "content": "stale" }),
        ];

        repair_model_history(&mut messages);

        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0]["role"], json!("user"));
    }

    #[test]
    fn repair_model_history_removes_unanswered_tool_calls_and_empty_assistants() {
        let mut messages = vec![
            json!({
                "role": "assistant",
                "content": "",
                "tool_calls": [
                    { "id": "c1", "type": "function", "function": { "name": "read_file", "arguments": "{}" } }
                ],
            }),
            json!({ "role": "user", "content": "next" }),
            json!({
                "role": "assistant",
                "content": "partial",
                "tool_calls": [
                    { "id": "c2", "type": "function", "function": { "name": "read_file", "arguments": "{}" } },
                    { "id": "c3", "type": "function", "function": { "name": "write_file", "arguments": "{}" } },
                ],
            }),
            json!({ "role": "tool", "tool_call_id": "c2", "content": "ok" }),
        ];

        repair_model_history(&mut messages);

        // The empty-content assistant whose only call went unanswered is gone;
        // the answered call survives and the unanswered one is trimmed.
        assert_eq!(messages.len(), 3);
        assert_eq!(messages[1]["tool_calls"].as_array().unwrap().len(), 1);
        assert_eq!(messages[1]["tool_calls"][0]["id"], json!("c2"));
        assert_eq!(messages[2]["tool_call_id"], json!("c2"));
    }

    #[test]
    fn repair_model_history_merges_split_assistant_tool_call_messages() {
        let mut messages = vec![
            json!({
                "role": "assistant",
                "content": "first",
                "tool_calls": [
                    { "id": "c1", "type": "function", "function": { "name": "read_file", "arguments": "{}" } }
                ],
            }),
            json!({
                "role": "assistant",
                "content": "second",
                "reasoning_content": "kept",
                "tool_calls": [
                    { "id": "c2", "type": "function", "function": { "name": "write_file", "arguments": "{}" } }
                ],
            }),
            json!({ "role": "tool", "tool_call_id": "c1", "content": "one" }),
            json!({ "role": "tool", "tool_call_id": "c2", "content": "two" }),
        ];

        repair_model_history(&mut messages);

        assert_eq!(messages.len(), 3);
        let merged = &messages[0];
        assert_eq!(merged["tool_calls"].as_array().unwrap().len(), 2);
        assert_eq!(merged["content"], json!("first\nsecond"));
        assert_eq!(merged["reasoning_content"], json!("kept"));
    }

    #[test]
    fn failed_edit_tool_result_requests_recovery() {
        let tool_call = json!({
            "function": {
                "name": "apply_patch",
                "arguments": "{\"patchText\":\"diff --git a/a b/a\"}"
            }
        });
        let failed_result = json!({
            "role": "tool",
            "content": "Tool apply_patch failed: git apply failed: patch does not apply"
        });
        let successful_result = json!({
            "role": "tool",
            "content": "Patch applied to files:\na"
        });

        assert!(ValidationOps::edit_needs_recovery(
            &tool_call,
            &failed_result
        ));
        assert!(!ValidationOps::edit_needs_recovery(
            &tool_call,
            &successful_result
        ));
    }

    #[test]
    fn edit_recovery_forces_the_next_supported_tool_call() {
        let state = AgentTurnState::new(false, false, false);
        assert_eq!(
            state.tool_choice(AgentTurnPhase::Conversation, false, true, false),
            Some(json!("required"))
        );
    }

    #[test]
    fn edit_recovery_persists_through_reads_and_clears_after_a_successful_edit() {
        assert_eq!(
            ValidationOps::next_recovery_state(false, 0, true, false),
            (true, 0)
        );
        assert_eq!(
            ValidationOps::next_recovery_state(true, 0, false, false),
            (true, 1)
        );
        assert_eq!(
            ValidationOps::next_recovery_state(true, 1, false, true),
            (false, 0)
        );
        assert_eq!(
            ValidationOps::next_recovery_state(
                true,
                MAX_EDIT_RECOVERY_TOOL_ROUNDS - 1,
                false,
                false,
            ),
            (false, MAX_EDIT_RECOVERY_TOOL_ROUNDS)
        );
    }

    #[test]
    fn edit_recovery_takes_priority_over_validation() {
        let mut validation = ValidationState::default();
        validation.mark_successful_edit();

        assert!(!validation.requires_tool(true));
        assert!(validation.requires_tool(false));
    }

    #[test]
    fn validation_instructions_require_repair_instead_of_early_final_answer() {
        let required = VALIDATION_REQUIRED_INSTRUCTION.to_ascii_lowercase();
        let unavailable = VALIDATION_UNAVAILABLE_INSTRUCTION.to_ascii_lowercase();

        assert!(required.contains("fix"));
        assert!(required.contains("until it passes"));
        assert!(!VALIDATION_UNAVAILABLE_INSTRUCTION.contains("Write the final answer now"));
        assert!(unavailable.contains("inspect"));
    }

    #[test]
    fn openai_reasoning_models_use_responses_api() {
        let request = ChatCompletionRequest {
            provider_name: "ChatGPT".to_string(),
            base_url: "http://127.0.0.1:15721/codex/v1".to_string(),
            api_key: "test".to_string(),
            model: "gpt-5.5".to_string(),
            wire_api: None,
            reasoning_effort: Some("high".to_string()),
            temperature: Some(0.7),
            system_prompt: None,
            workspace_path: None,
            code_tools_enabled: None,
            orchestration_tools_enabled: None,
            orchestration_required: None,
            can_write: None,
            stream_id: None,
            cancellation_id: None,
            messages: vec![],
        };

        assert!(should_use_responses_api(&request, false));
        assert!(!should_use_responses_api(&request, true));

        let mut chat_wire_request = request;
        chat_wire_request.base_url = "https://relay.example.com/v1".to_string();
        chat_wire_request.wire_api = Some("chat".to_string());
        assert!(!should_use_responses_api(&chat_wire_request, false));

        let mut unknown_proxy_request = chat_wire_request;
        unknown_proxy_request.wire_api = None;
        assert!(!should_use_responses_api(&unknown_proxy_request, false));
    }

    #[test]
    fn responses_reasoning_payload_requests_summary_when_enabled() {
        assert_eq!(
            responses_reasoning_payload(Some("medium")),
            Some(json!({ "effort": "medium", "summary": "auto" }))
        );
        assert_eq!(responses_reasoning_payload(Some("off")), None);
    }

    #[test]
    fn responses_endpoint_accepts_base_chat_or_full_endpoint() {
        assert_eq!(
            responses_endpoint("https://api.example.com/v1"),
            "https://api.example.com/v1/responses"
        );
        assert_eq!(
            responses_endpoint("https://api.example.com/v1/chat/completions"),
            "https://api.example.com/v1/responses"
        );
        assert_eq!(
            responses_endpoint("https://api.example.com/v1/responses"),
            "https://api.example.com/v1/responses"
        );
    }

    #[test]
    fn parses_responses_text_reasoning_and_usage() {
        let response = json!({
            "id": "resp_123",
            "output_text": "final answer",
            "output": [
                {
                    "type": "reasoning",
                    "summary": [
                        { "type": "summary_text", "text": "checked the workspace" }
                    ]
                }
            ],
            "usage": {
                "input_tokens": 100,
                "output_tokens": 25,
                "total_tokens": 125,
                "input_tokens_details": { "cached_tokens": 40 }
            }
        });

        assert_eq!(responses_id(&response).as_deref(), Some("resp_123"));
        assert_eq!(responses_output_text(&response), "final answer");
        assert_eq!(
            responses_reasoning_trace_steps(&response)[0].text,
            "checked the workspace"
        );

        let usage = TraceCtx::usage_from(&response).unwrap();
        assert_eq!(usage.prompt_tokens, Some(100));
        assert_eq!(usage.completion_tokens, Some(25));
        assert_eq!(usage.prompt_cache_hit_tokens, Some(40));
        assert_eq!(usage.prompt_cache_miss_tokens, Some(60));
    }

    #[test]
    fn appends_responses_reasoning_stream_without_done_duplicates() {
        let mut reasoning = String::new();

        assert_eq!(
            append_response_reasoning(&mut reasoning, "checked", false).as_deref(),
            Some("checked")
        );
        assert_eq!(
            append_response_reasoning(&mut reasoning, "checked", true),
            None
        );
        assert_eq!(
            append_response_reasoning(&mut reasoning, "checked workspace", true).as_deref(),
            Some(" workspace")
        );
        assert_eq!(
            append_response_reasoning(&mut reasoning, "opened files", true).as_deref(),
            Some("\nopened files")
        );
        assert_eq!(reasoning, "checked workspace\nopened files");
    }

    #[test]
    fn extracts_responses_reasoning_stream_text_shapes() {
        assert_eq!(response_stream_text(&json!({ "delta": "a" })), Some("a"));
        assert_eq!(response_stream_text(&json!({ "text": "b" })), Some("b"));
        assert_eq!(
            response_stream_text(&json!({ "part": { "type": "summary_text", "text": "c" } })),
            Some("c")
        );
    }

    #[test]
    fn converts_responses_function_calls_to_chat_tools_and_outputs() {
        let response_call = json!({
            "type": "function_call",
            "call_id": "call_123",
            "name": "read_file",
            "arguments": "{\"file\":\"src/lib.rs\"}"
        });
        let tool_call = response_function_call_to_chat_tool_call(&response_call);

        assert_eq!(tool_call["id"], json!("call_123"));
        assert_eq!(tool_call["function"]["name"], json!("read_file"));

        let tool_output = response_tool_output(
            &tool_call,
            &json!({
                "role": "tool",
                "tool_call_id": "call_123",
                "content": "file contents"
            }),
        );

        assert_eq!(tool_output["type"], json!("function_call_output"));
        assert_eq!(tool_output["call_id"], json!("call_123"));
        assert_eq!(tool_output["output"], json!("file contents"));
    }

    #[test]
    fn final_answer_request_appends_internal_instruction() {
        let messages = vec![json!({ "role": "user", "content": "question" })];
        let payload_messages = chat_payload_messages(&messages, true, false, None);

        assert_eq!(payload_messages.len(), 2);
        assert_eq!(payload_messages[0]["content"], json!("question"));
        assert_eq!(payload_messages[1]["role"], json!("user"));
        assert!(payload_messages[1]["content"]
            .as_str()
            .unwrap()
            .contains("final answer"));
    }

    #[test]
    fn validation_request_takes_priority_over_final_answer_instruction() {
        let messages = vec![json!({ "role": "user", "content": "question" })];
        let payload_messages = chat_payload_messages(&messages, true, true, None);

        assert_eq!(payload_messages.len(), 2);
        assert!(payload_messages[1]["content"]
            .as_str()
            .unwrap()
            .to_ascii_lowercase()
            .contains("call run_command"));
        assert!(!payload_messages[1]["content"]
            .as_str()
            .unwrap()
            .contains(FINAL_ANSWER_INSTRUCTION));
    }

    #[test]
    fn checkpoint_request_takes_priority_over_final_answer_instruction() {
        let messages = vec![json!({ "role": "user", "content": "question" })];
        let payload_messages = chat_payload_messages(
            &messages,
            true,
            false,
            Some(TOOL_CALL_CHECKPOINT_INSTRUCTION),
        );

        assert_eq!(payload_messages.len(), 2);
        assert!(payload_messages[1]["content"]
            .as_str()
            .unwrap()
            .contains("do not call any tools"));
        assert!(!payload_messages[1]["content"]
            .as_str()
            .unwrap()
            .contains(FINAL_ANSWER_INSTRUCTION));
    }

    #[test]
    fn tool_action_request_appends_internal_instruction() {
        let messages = vec![json!({ "role": "user", "content": "question" })];
        let payload_messages = chat_payload_messages(
            &messages,
            false,
            false,
            Some(DEEPSEEK_TOOL_ACTION_INSTRUCTION),
        );

        assert_eq!(payload_messages.len(), 2);
        assert_eq!(payload_messages[0]["content"], json!("question"));
        assert_eq!(payload_messages[1]["role"], json!("user"));
        assert_eq!(
            payload_messages[1]["content"],
            json!(DEEPSEEK_TOOL_ACTION_INSTRUCTION)
        );
    }

    #[test]
    fn dispatched_tasks_completion_content_lists_assignments() {
        let entries = vec![
            TaskDispatchedEntry {
                member: "Silver Wolf".to_string(),
                instruction: "Inspect the log loop".to_string(),
            },
            TaskDispatchedEntry {
                member: "Kafka".to_string(),
                instruction: "Summarize the worker findings".to_string(),
            },
        ];

        assert_eq!(
            dispatched_tasks_completion_content(&entries),
            "- @Silver Wolf: Inspect the log loop\n- @Kafka: Summarize the worker findings"
        );
    }

    #[test]
    fn dispatch_tasks_can_finish_without_reflection_when_validation_is_clear() {
        let validation = ValidationState::default();
        let entries = vec![TaskDispatchedEntry {
            member: "Silver Wolf".to_string(),
            instruction: "Inspect the log loop".to_string(),
        }];

        assert!(should_finish_after_dispatch_tasks(
            Some(entries.as_slice()),
            &validation,
            false
        ));

        let mut pending_validation = ValidationState::default();
        pending_validation.mark_successful_edit();
        assert!(!should_finish_after_dispatch_tasks(
            Some(entries.as_slice()),
            &pending_validation,
            false
        ));

        assert!(!should_finish_after_dispatch_tasks(
            Some(entries.as_slice()),
            &validation,
            true
        ));
    }

    #[test]
    fn extracts_message_content_from_string_and_text_parts() {
        assert_eq!(
            StrUtils::message_content_text(&json!({ "content": "  hello  " })),
            "hello"
        );
        assert_eq!(
            StrUtils::message_content_text(&json!({
                "content": [
                    { "type": "text", "text": "hello" },
                    { "type": "text", "text": "world" }
                ]
            })),
            "hello\nworld"
        );
    }

    #[test]
    fn extracts_deepseek_reasoning_content() {
        assert_eq!(
            TraceCtx::message_reasoning(&json!({
                "reasoning_content": "  line one\nline two  ",
                "content": "answer"
            })),
            "line one\nline two"
        );
    }

    #[test]
    fn extracts_deepseek_prompt_cache_usage() {
        let usage = TraceCtx::usage_from(&json!({
            "usage": {
                "prompt_tokens": 120,
                "completion_tokens": 30,
                "total_tokens": 150,
                "prompt_cache_hit_tokens": 90,
                "prompt_cache_miss_tokens": 30
            }
        }))
        .expect("usage should parse");

        assert_eq!(usage.prompt_tokens, Some(120));
        assert_eq!(usage.prompt_cache_hit_tokens, Some(90));
        assert_eq!(usage.prompt_cache_miss_tokens, Some(30));
    }

    #[test]
    fn code_tools_schema_includes_file_search_tools() {
        let schema = code_tools_schema(true, true);
        let names = schema
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|tool| tool.get("function")?.get("name")?.as_str())
            .collect::<Vec<_>>();

        assert!(names.contains(&"codegraph_explore"));
        assert!(names.contains(&"codegraph_command"));
        assert!(names.contains(&"read_file"));
        assert!(names.contains(&"list_files"));
        assert!(names.contains(&"search_files"));
        assert!(names.contains(&"glob_files"));
        assert!(names.contains(&"write_file"));
        assert!(names.contains(&"create_directory"));
        assert!(names.contains(&"delete_path"));
        assert!(names.contains(&"move_path"));
        assert!(names.contains(&"apply_patch"));
        assert!(names.contains(&"run_command"));

        let apply_patch_tool = schema
            .as_array()
            .unwrap()
            .iter()
            .find(|tool| {
                tool.get("function")
                    .and_then(|function| function.get("name"))
                    .and_then(Value::as_str)
                    == Some("apply_patch")
            })
            .expect("apply_patch tool should be present");
        let apply_patch_description = apply_patch_tool["function"]["description"]
            .as_str()
            .expect("apply_patch should describe safe patch construction");
        assert!(apply_patch_description.contains("read the exact current target location"));
        assert!(apply_patch_description.contains("hand-guessed line numbers"));
        assert!(apply_patch_description.contains("checkOnly=true"));
    }

    #[test]
    fn code_tools_schema_hides_write_tools_without_permission() {
        let schema = code_tools_schema(true, false);
        let names = schema
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|tool| tool.get("function")?.get("name")?.as_str())
            .collect::<Vec<_>>();

        assert!(names.contains(&"codegraph_explore"));
        assert!(names.contains(&"codegraph_command"));
        assert!(names.contains(&"read_file"));
        assert!(names.contains(&"list_files"));
        assert!(names.contains(&"search_files"));
        assert!(names.contains(&"glob_files"));
        assert!(!names.contains(&"write_file"));
        assert!(!names.contains(&"create_directory"));
        assert!(!names.contains(&"delete_path"));
        assert!(!names.contains(&"move_path"));
        assert!(!names.contains(&"apply_patch"));
        assert!(!names.contains(&"run_command"));

        let codegraph_commands = schema
            .as_array()
            .unwrap()
            .iter()
            .find(|tool| tool["function"]["name"] == json!("codegraph_command"))
            .and_then(|tool| {
                tool["function"]["parameters"]["properties"]["command"]["enum"].as_array()
            })
            .unwrap();
        assert!(codegraph_commands.contains(&json!("status")));
        assert!(!codegraph_commands.contains(&json!("sync")));
    }

    #[test]
    fn execute_code_tool_call_blocks_write_tools_without_permission() {
        let tool_result = execute_code_tool_call(
            Path::new("."),
            &json!({
                "id": "call-run",
                "function": {
                    "name": "run_command",
                    "arguments": "{\"command\":\"echo\",\"args\":[\"should-not-run\"]}"
                }
            }),
            false,
            None,
        );

        let content = tool_result
            .get("content")
            .and_then(Value::as_str)
            .unwrap_or_default();

        assert!(content.contains("write permission is disabled"));
        assert!(!content.contains("should-not-run"));
    }

    #[test]
    fn execute_code_tool_call_blocks_codegraph_updates_without_permission() {
        let tool_result = execute_code_tool_call(
            Path::new("."),
            &json!({
                "id": "call-codegraph-sync",
                "function": {
                    "name": "codegraph_command",
                    "arguments": "{\"command\":\"sync\"}"
                }
            }),
            false,
            None,
        );

        let content = tool_result
            .get("content")
            .and_then(Value::as_str)
            .unwrap_or_default();

        assert!(content.contains("write permission is disabled"));
    }

    #[test]
    fn read_file_tool_reads_line_ranges_and_blocks_traversal() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let workspace = std::env::temp_dir().join(format!("matrix-tool-test-{}", stamp));
        let src_dir = workspace.join("src");
        fs::create_dir_all(&src_dir).unwrap();
        fs::write(src_dir.join("main.rs"), "one\ntwo\nthree\n").unwrap();
        let workspace = fs::canonicalize(&workspace).unwrap();

        let content = read_workspace_file_tool(
            &workspace,
            &json!({ "file": "src/main.rs", "startLine": 2, "maxLines": 1 }),
        )
        .unwrap();

        assert!(content.contains("2\ttwo"));
        assert!(resolve_workspace_relative_path(&workspace, "../outside").is_err());

        let _ = fs::remove_dir_all(&workspace);
    }

    #[test]
    fn file_write_move_and_delete_tools_stay_in_workspace() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let workspace = std::env::temp_dir().join(format!("matrix-write-tool-test-{}", stamp));
        fs::create_dir_all(&workspace).unwrap();
        let workspace = fs::canonicalize(&workspace).unwrap();

        write_workspace_file_tool(
            &workspace,
            &json!({
                "file": "notes/one.txt",
                "content": "hello",
                "mode": "create"
            }),
        )
        .unwrap();
        assert!(workspace.join("notes/one.txt").is_file());

        move_workspace_path_tool(
            &workspace,
            &json!({
                "from": "notes/one.txt",
                "to": "notes/two.txt"
            }),
        )
        .unwrap();
        assert!(workspace.join("notes/two.txt").is_file());

        delete_workspace_path_tool(
            &workspace,
            &json!({
                "path": "notes/two.txt"
            }),
        )
        .unwrap();
        assert!(!workspace.join("notes/two.txt").exists());
        assert!(write_workspace_file_tool(
            &workspace,
            &json!({
                "file": "../outside.txt",
                "content": "no"
            }),
        )
        .is_err());

        let _ = fs::remove_dir_all(&workspace);
    }

    #[test]
    fn appends_reasoning_trace_lines() {
        let mut trace_steps = Vec::new();

        TraceCtx::append_reasoning(
            &mut trace_steps,
            &json!({ "reasoning_content": "first\n\nsecond" }),
        );

        assert_eq!(trace_steps.len(), 2);
        assert_eq!(trace_steps[0].kind, "reasoning");
        assert_eq!(trace_steps[0].text, "first");
        assert_eq!(trace_steps[1].text, "second");
    }

    #[test]
    fn describes_codegraph_tool_call_and_result() {
        let tool_call = json!({
            "function": {
                "name": "codegraph_explore",
                "arguments": "{\"query\":\"ChatGroupPage reasoning\",\"maxFiles\":4}"
            }
        });
        let tool_message = json!({
            "role": "tool",
            "content": "CodeGraph explore note:\nFound 8 symbols across 2 files."
        });

        let call_step = tool_call_trace_step(&tool_call);
        let result_step = tool_result_trace_step(&tool_call, &tool_message);

        assert_eq!(call_step.kind, "tool");
        assert!(call_step.text.contains("ChatGroupPage reasoning"));
        assert!(call_step.text.contains("maxFiles=4"));
        assert!(result_step.text.contains("via CodeGraph"));
    }

    #[test]
    fn finds_sse_event_separators() {
        assert_eq!(sse_event_separator("data: one\n\nrest"), Some((9, 2)));
        assert_eq!(sse_event_separator("data: one\r\n\r\nrest"), Some((9, 4)));
        assert_eq!(sse_event_separator("data: one"), None);
    }

    #[test]
    fn accumulates_streamed_tool_call_arguments() {
        let mut tool_calls = Vec::new();

        append_delta_tool_calls(
            &[json!({
                "index": 0,
                "id": "call_1",
                "type": "function",
                "function": {
                    "name": "codegraph_",
                    "arguments": "{\"query\":\"Chat"
                }
            })],
            &mut tool_calls,
        );
        append_delta_tool_calls(
            &[json!({
                "index": 0,
                "function": {
                    "name": "explore",
                    "arguments": "GroupPage\"}"
                }
            })],
            &mut tool_calls,
        );

        let values = tool_call_accumulators_to_values(tool_calls);

        assert_eq!(values.len(), 1);
        assert_eq!(values[0]["id"], json!("call_1"));
        assert_eq!(values[0]["function"]["name"], json!("codegraph_explore"));
        assert_eq!(
            values[0]["function"]["arguments"],
            json!("{\"query\":\"ChatGroupPage\"}")
        );
    }

    #[test]
    fn chat_endpoint_accepts_base_or_full_endpoint() {
        assert_eq!(
            chat_completions_endpoint("https://api.example.com/v1"),
            "https://api.example.com/v1/chat/completions"
        );
        assert_eq!(
            chat_completions_endpoint("https://api.example.com/v1/chat/completions"),
            "https://api.example.com/v1/chat/completions"
        );
    }

    #[test]
    fn chat_endpoint_normalizes_ccswitch_codex_proxy_base() {
        assert_eq!(
            chat_completions_endpoint("http://127.0.0.1:15721/codex"),
            "http://127.0.0.1:15721/codex/v1/chat/completions"
        );
    }

    #[test]
    fn builds_openai_config_from_codex_toml_and_auth() {
        let config = build_ccswitch_openai_config(
            "test".to_string(),
            None,
            r#"
model_provider = "custom"
model = "gpt-5.5"

[model_providers.custom]
name = "Relay"
base_url = "https://relay.example.com/v1"
wire_api = "chat"
"#,
            Some(&json!({ "OPENAI_API_KEY": "sk-test" })),
            None,
            None,
        )
        .unwrap();

        assert_eq!(config.provider_name.as_deref(), Some("Relay"));
        assert_eq!(config.base_url, "https://relay.example.com/v1");
        assert_eq!(config.api_key, "sk-test");
        assert_eq!(config.model.as_deref(), Some("gpt-5.5"));
        assert_eq!(config.wire_api.as_deref(), Some("chat"));
        assert!(config.warning.is_none());
    }

    #[test]
    fn reads_first_choice_finish_reason() {
        let parsed = json!({
            "choices": [
                {
                    "finish_reason": "tool_calls",
                    "message": { "content": "" }
                }
            ]
        });

        assert_eq!(
            first_choice_finish_reason(&parsed),
            Some("tool_calls".to_string())
        );
    }

    #[test]
    fn detects_truncated_finish_reasons() {
        assert!(finish_reason_indicates_truncated_output(Some("length")));
        assert!(finish_reason_indicates_truncated_output(Some("MAX_TOKENS")));
        assert!(!finish_reason_indicates_truncated_output(Some("stop")));
        assert!(!finish_reason_indicates_truncated_output(None));
    }

    #[test]
    fn detects_incomplete_responses_output() {
        assert!(responses_output_is_incomplete(&json!({
            "status": "incomplete"
        })));
        assert!(responses_output_is_incomplete(&json!({
            "incomplete_details": { "reason": "max_output_tokens" }
        })));
        assert!(!responses_output_is_incomplete(&json!({
            "status": "completed"
        })));
    }

    #[test]
    fn combines_continued_output_segments() {
        let mut accumulated = String::new();
        append_continued_output(&mut accumulated, "Part one");
        append_continued_output(&mut accumulated, "Part two");

        assert_eq!(combined_output_text(&accumulated, "Part three"), "Part one\n\nPart two\n\nPart three");
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(types::ChatCancellationState::default())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            cache::load_app_cache,
            cache::save_app_cache,
            cache::copy_avatar_to_cache,
            config::load_ccswitch_openai_config,
            probe::probe_model_provider,
            chat::chat_completion,
            chat::cancel_chat_completion,
            chat::finish_chat_completion,
            tools::codegraph::inspect_code_workspace,
            tools::patch::apply_patch_proposal
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
