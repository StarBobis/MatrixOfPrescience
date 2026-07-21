use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, State};

use crate::*;

pub(crate) fn chat_completions_endpoint(base_url: &str) -> String {
    let normalized = normalize_imported_base_url(base_url);
    let lower = normalized.to_ascii_lowercase();

    if lower.ends_with("/chat/completions") {
        return normalized;
    }

    format!("{}/chat/completions", normalized.trim_end_matches('/'))
}

pub(crate) fn responses_endpoint(base_url: &str) -> String {
    let normalized = normalize_imported_base_url(base_url);
    let trimmed = normalized.trim_end_matches('/');
    let lower = trimmed.to_ascii_lowercase();

    if lower.ends_with("/responses") {
        return trimmed.to_string();
    }

    if lower.ends_with("/chat/completions") {
        let base = &trimmed[..trimmed.len() - "/chat/completions".len()];
        return format!("{}/responses", base.trim_end_matches('/'));
    }

    format!("{}/responses", trimmed)
}

pub(crate) fn is_openai_reasoning_model(model: &str) -> bool {
    let model = model.trim().to_ascii_lowercase();
    model.starts_with("gpt-5")
        || model.starts_with("o1")
        || model.starts_with("o3")
        || model.starts_with("o4")
}

pub(crate) fn should_use_responses_api(request: &ChatCompletionRequest, is_deepseek: bool) -> bool {
    if is_deepseek
        || !reasoning_enabled(request.reasoning_effort.as_deref())
        || !is_openai_reasoning_model(&request.model)
    {
        return false;
    }

    let wire_api = request
        .wire_api
        .as_deref()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();

    if wire_api.contains("chat") {
        return false;
    }

    wire_api.contains("responses")
        || is_local_ccswitch_proxy_url(&request.base_url)
        || request
            .base_url
            .to_ascii_lowercase()
            .contains("api.openai.com")
}

#[tauri::command]
pub async fn chat_completion(
    app: AppHandle,
    request: ChatCompletionRequest,
    cancellation_state: State<'_, ChatCancellationState>,
) -> Result<ChatCompletionResponse, String> {
    if request.api_key.trim().is_empty() {
        return Err(format!(
            "{} API Key is not configured.",
            request.provider_name
        ));
    }

    if request.model.trim().is_empty() {
        return Err("Model name cannot be empty.".to_string());
    }

    let code_workspace = if request.code_tools_enabled.unwrap_or(false) {
        Some(validate_workspace(
            request.workspace_path.as_deref().unwrap_or(""),
        )?)
    } else {
        None
    };
    let cancellation = request
        .cancellation_id
        .as_deref()
        .map(|cancellation_id| cancellation_state.token(cancellation_id));

    let mut messages: Vec<Value> = Vec::new();
    let mut system_prompt_for_trace: Option<String> = None;
    if let Some(system_prompt) = request.system_prompt.as_deref() {
        let trimmed = system_prompt.trim();
        if !trimmed.is_empty() {
            system_prompt_for_trace = Some(trimmed.to_string());
            messages.push(json!({
                "role": "system",
                "content": trimmed,
            }));
        }
    }

    for message in &request.messages {
        let mut msg = json!({
            "role": message.role,
            "content": message.content,
        });
        if let Some(reasoning) = &message.reasoning_content {
            if !reasoning.trim().is_empty() {
                msg["reasoning_content"] = json!(reasoning);
            }
        }
        messages.push(msg);
    }

    let is_deepseek = is_deepseek_provider(&request.provider_name, &request.base_url);

    if should_use_responses_api(&request, is_deepseek) {
        return openai_responses_completion(app, request, code_workspace, cancellation).await;
    }

    let endpoint = chat_completions_endpoint(&request.base_url);
    let client = reqwest::Client::new();
    let deepseek_reasoning_requested =
        is_deepseek && reasoning_enabled(request.reasoning_effort.as_deref());
    // The reflection/action ping-pong workflow was retired: weak models ramble
    // through the FINAL/CONTINUE protocol instead of acting, so every phase
    // transition became another chance to stall. The plain loop below (tools
    // offered every turn; whatever the model says once it stops calling tools
    // is the answer) combined with the no-progress guard and the hard stall
    // termination is strictly more robust. AgentTurnState keeps the workflow
    // machinery for its phase/tool_choice helpers.
    let deepseek_tool_workflow = false;
    let mut agent_turn_state = AgentTurnState::new(
        is_deepseek,
        deepseek_reasoning_requested,
        deepseek_tool_workflow,
    );
    let max_chat_completion_turns = agent_turn_state.max_turns(MAX_CHAT_COMPLETION_TURNS);
    let can_write = request.can_write.unwrap_or(false);
    let max_tool_calls_per_turn = if is_deepseek {
        MAX_DEEPSEEK_TOOL_CALLS_PER_TURN
    } else {
        TOOL_CALL_CHECKPOINT_INTERVAL
    };
    let mut code_tool_called = false;
    let mut edit_recovery_required = false;
    let mut edit_recovery_rounds = 0usize;
    let mut final_answer_requested = false;
    let mut tool_only_rounds: usize = 0;
    let mut unproductive_turns: usize = 0;
    let mut tool_calls_since_checkpoint = 0usize;
    let mut total_tool_calls_executed = 0usize;
    let mut dispatched_tasks: Option<Vec<TaskDispatchedEntry>> = None;
    let mut tool_checkpoint_pending = false;
    let mut tool_budget_reset_pending = false;
    let mut validation = ValidationState::default();
    let mut no_progress_guard = NoProgressGuard::default();
    let mut last_finish_reason: Option<String> = None;
    let mut last_usage: Option<ChatCompletionUsage> = None;
    let mut trace_steps: Vec<ChatTraceStep> = Vec::new();
    let mut last_payload_instruction: Option<&'static str> = None;
    let mut accumulated_display_content = String::new();
    let stream_id = request
        .stream_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);

    if let Some(prompt) = system_prompt_for_trace.as_deref() {
        emit_instruction_step(&app, stream_id.as_deref(), &mut trace_steps, "system-prompt", prompt);
    }

    for turn_index in 0..max_chat_completion_turns {
        let validation_tool_required = validation.requires_tool(edit_recovery_required);
        if validation_tool_required {
            validation.mark_model_prompted();
        }
        let repair_required = edit_recovery_required || validation.requires_repair();
        let turn_phase = agent_turn_state.phase(
            final_answer_requested,
            tool_checkpoint_pending,
            validation_tool_required,
            repair_required,
        );
        let checkpoint_required = matches!(
            turn_phase,
            AgentTurnPhase::ToolReflection | AgentTurnPhase::BudgetCheckpoint
        );
        let phase_instruction = match turn_phase {
            AgentTurnPhase::ToolReflection => Some(AgentTurnState::DEEPSEEK_REFLECTION_INSTRUCTION),
            AgentTurnPhase::BudgetCheckpoint => Some(TOOL_CALL_CHECKPOINT_INSTRUCTION),
            AgentTurnPhase::ToolAction if deepseek_tool_workflow => {
                Some(DEEPSEEK_TOOL_ACTION_INSTRUCTION)
            }
            _ => None,
        };
        // Kun's model-history repair first: the payload sent upstream must be
        // structurally legal (no orphan tool results, no unanswered tool
        // calls, no split assistant tool_call messages) for every provider.
        repair_model_history(&mut messages);
        // DeepSeek thinking mode additionally requires every assistant
        // message that made tool calls to carry reasoning_content in all
        // subsequent requests (HTTP 400 otherwise).
        if is_deepseek && deepseek_reasoning_requested {
            backfill_deepseek_reasoning(&mut messages);
        }
        let payload_messages = chat_payload_messages(
            &messages,
            final_answer_requested && !validation_tool_required && !repair_required,
            validation_tool_required,
            phase_instruction,
        );
        let payload_instruction = if validation_tool_required {
            Some(("validation-required", VALIDATION_REQUIRED_INSTRUCTION))
        } else if let Some(instruction) = phase_instruction {
            Some(("turn-phase", instruction))
        } else if final_answer_requested && !validation_tool_required && !repair_required {
            Some(("final-answer", FINAL_ANSWER_INSTRUCTION))
        } else {
            None
        };
        // Surface each newly injected instruction, skipping consecutive repeats
        // so a deepseek action phase does not flood the trace.
        if let Some((label, instruction)) = payload_instruction {
            if last_payload_instruction != Some(instruction) {
                emit_instruction_step(&app, stream_id.as_deref(), &mut trace_steps, label, instruction);
                last_payload_instruction = Some(instruction);
            }
        } else {
            last_payload_instruction = None;
        }
        let suppress_content_stream = is_deepseek
            && deepseek_tool_workflow
            && !matches!(
                turn_phase,
                AgentTurnPhase::Conversation | AgentTurnPhase::FinalAnswer
            );
        let mut payload = json!({
            "model": request.model,
            "messages": payload_messages,
            "temperature": request.temperature.unwrap_or(0.7),
        });
        let tools_blocked = turn_phase.blocks_tools();
        let orchestration_required = request.orchestration_required.unwrap_or(false);
        let code_tools_allowed = !tools_blocked
            && code_workspace.is_some()
            && !orchestration_required;
        let orchestration_tools_allowed =
            !tools_blocked && request.orchestration_tools_enabled.unwrap_or(false);
        let any_tools_allowed = code_tools_allowed || orchestration_tools_allowed;
        let strict_tool_schema = is_deepseek && any_tools_allowed;

        let reasoning_effort = agent_turn_state.reasoning_effort(
            turn_phase,
            request.reasoning_effort.as_deref(),
            TOOL_CALL_CHECKPOINT_REASONING_EFFORT,
        );
        apply_reasoning_payload(&mut payload, is_deepseek, reasoning_effort);
        if turn_phase == AgentTurnPhase::ToolReflection {
            payload["max_tokens"] = json!(AgentTurnState::DEEPSEEK_REFLECTION_MAX_TOKENS);
        }

        if any_tools_allowed {
            let mut tools: Vec<Value> = Vec::new();
            if code_tools_allowed {
                if let Value::Array(code_tools) = code_tools_schema(is_deepseek, can_write) {
                    tools.extend(code_tools);
                }
            }
            if orchestration_tools_allowed {
                if let Value::Array(orchestration_tools) = orchestration_tools_schema(is_deepseek) {
                    tools.extend(orchestration_tools);
                }
            }
            // No-progress escalation: remove read/explore tools so the model
            // must edit, run a command, or answer instead of stalling.
            if no_progress_guard.read_tools_blocked() {
                retain_non_read_tools(&mut tools);
            }
            if !tools.is_empty() {
                payload["tools"] = Value::Array(tools);
                if let Some(tool_choice) = agent_turn_state.tool_choice(
                    turn_phase,
                    validation_tool_required,
                    repair_required,
                    orchestration_required,
                ) {
                    payload["tool_choice"] = tool_choice;
                }
            }
        }

        let parsed = match send_chat_completion_request_maybe_stream(
            &app,
            stream_id.as_deref(),
            &client,
            &endpoint,
            &request.api_key,
            &request.provider_name,
            &payload,
            cancellation.as_deref(),
            suppress_content_stream,
        )
        .await
        {
            Ok(parsed) => parsed,
            Err(error) if strict_tool_schema => {
                let mut fallback_tools: Vec<Value> = Vec::new();
                if code_tools_allowed {
                    if let Value::Array(code_tools) = code_tools_schema(false, can_write) {
                        fallback_tools.extend(code_tools);
                    }
                }
                if orchestration_tools_allowed {
                    if let Value::Array(orchestration_tools) = orchestration_tools_schema(false) {
                        fallback_tools.extend(orchestration_tools);
                    }
                }
                if no_progress_guard.read_tools_blocked() {
                    retain_non_read_tools(&mut fallback_tools);
                }
                if fallback_tools.is_empty() {
                    if let Some(object) = payload.as_object_mut() {
                        object.remove("tools");
                        object.remove("tool_choice");
                    }
                } else {
                    payload["tools"] = Value::Array(fallback_tools);
                    if let Some(tool_choice) = agent_turn_state.tool_choice(
                        turn_phase,
                        validation_tool_required,
                        repair_required,
                        orchestration_required,
                    ) {
                        payload["tool_choice"] = tool_choice;
                    } else {
                        if let Some(object) = payload.as_object_mut() {
                            object.remove("tool_choice");
                        }
                    }
                }
                send_chat_completion_request_maybe_stream(
                    &app,
                    stream_id.as_deref(),
                    &client,
                    &endpoint,
                    &request.api_key,
                    &request.provider_name,
                    &payload,
                    cancellation.as_deref(),
                    suppress_content_stream,
                )
                .await
                .map_err(|fallback_error| {
                    format!(
                        "{}; fallback without strict tool schema also failed: {}",
                        error, fallback_error
                    )
                })?
            }
            Err(error) => return Err(error),
        };
        if let Some(usage) = TraceCtx::usage_from(&parsed) {
            last_usage = Some(usage);
        }
        last_finish_reason = first_choice_finish_reason(&parsed);
        let message = parsed
            .get("choices")
            .and_then(|choices| choices.get(0))
            .and_then(|choice| choice.get("message"))
            .cloned()
            .ok_or_else(|| format!("{} returned no message.", request.provider_name))?;
        let message = normalize_dsml_tool_calls_in_message(message);
        TraceCtx::append_reasoning(&mut trace_steps, &message);

        if !checkpoint_required {
            if let (Some(workspace), Some(tool_calls)) = (
                code_workspace.as_ref(),
                message.get("tool_calls").and_then(Value::as_array),
            ) {
                if !tool_calls.is_empty() {
                    let remaining_total_tool_budget = MAX_TOTAL_TOOL_CALLS_PER_AUTONOMOUS_ROUND
                        .saturating_sub(total_tool_calls_executed);
                    // Hard-cap a single assistant tool batch so one response cannot skip the checkpoint.
                    let tool_execution_plan = ToolCallUtils::plan_execution(
                        tool_calls.len(),
                        tool_calls_since_checkpoint,
                        TOOL_CALL_CHECKPOINT_INTERVAL,
                        max_tool_calls_per_turn,
                        remaining_total_tool_budget,
                    );
                    if tool_execution_plan.executable_count == 0 {
                        if ToolCallUtils::can_open_next_autonomous_round(
                            turn_index,
                            max_chat_completion_turns,
                        ) {
                            tool_checkpoint_pending = true;
                            tool_budget_reset_pending = true;
                            final_answer_requested = false;
                        } else {
                            tool_checkpoint_pending = false;
                            final_answer_requested = true;
                        }
                        continue;
                    }
                    let executable_tool_calls = &tool_calls[..tool_execution_plan.executable_count];
                    messages.push(ToolCallUtils::trim_chat_tool_calls(
                        &message,
                        tool_execution_plan.executable_count,
                    ));
                    let mut failed_edit = false;
                    let mut successful_edit = false;
                    let mut read_loop_nudge: Option<String> = None;
                    let mut validation_run = ValidationRun::default();
                    for tool_call in executable_tool_calls {
                        let call_step = tool_call_trace_step(tool_call);
                        emit_trace_step(&app, stream_id.as_deref(), &call_step);
                        TraceCtx::append_steps(&mut trace_steps, vec![call_step]);
                        let tool_result = if let Some(tool_result) =
                            validation.redundant_validation_tool_result(tool_call)
                        {
                            tool_result
                        } else {
                            let mut stream_tool_output = |step: ChatTraceStep| {
                                emit_tool_chunk(&app, stream_id.as_deref(), &step);
                            };
                            execute_code_tool_call(
                                workspace,
                                tool_call,
                                can_write,
                                Some(&mut stream_tool_output),
                            )
                        };
                        validation_run.observe_tool_result(tool_call, &tool_result);
                        if let Some(nudge) = no_progress_guard.record_tool_call(tool_call) {
                            read_loop_nudge = Some(nudge);
                        }
                        if dispatched_tasks.is_none() {
                            if let Some(entries) = extract_dispatched_tasks(tool_call, &tool_result) {
                                dispatched_tasks = Some(entries);
                            }
                        }
                        if ValidationOps::result_succeeded(&tool_result)
                            && ValidationOps::is_edit_call(tool_call)
                        {
                            successful_edit = true;
                        }
                        if ValidationOps::edit_needs_recovery(tool_call, &tool_result) {
                            failed_edit = true;
                        }
                        let result_step = tool_result_trace_step(tool_call, &tool_result);
                        emit_trace_step(&app, stream_id.as_deref(), &result_step);
                        TraceCtx::append_steps(&mut trace_steps, vec![result_step]);
                        messages.push(tool_result);
                    }
                    (edit_recovery_required, edit_recovery_rounds) =
                        ValidationOps::next_recovery_state(
                            edit_recovery_required,
                            edit_recovery_rounds,
                            failed_edit,
                            successful_edit,
                        );
                    if failed_edit && edit_recovery_required {
                        emit_instruction_step(&app, stream_id.as_deref(), &mut trace_steps, "edit-recovery", EDIT_FAILURE_RECOVERY_INSTRUCTION);
                        messages.push(json!({
                            "role": "user",
                            "content": EDIT_FAILURE_RECOVERY_INSTRUCTION,
                        }));
                    }
                    if let Some(nudge) = read_loop_nudge {
                        emit_instruction_step(&app, stream_id.as_deref(), &mut trace_steps, "no-progress-nudge", &nudge);
                        messages.push(json!({
                            "role": "user",
                            "content": nudge,
                        }));
                    }
                    let validation_failed = if successful_edit {
                        validation.mark_successful_edit();
                        false
                    } else {
                        validation.record_run(validation_run)
                    };
                    if validation_failed {
                        emit_instruction_step(&app, stream_id.as_deref(), &mut trace_steps, "validation-failure", VALIDATION_FAILURE_RECOVERY_INSTRUCTION);
                        messages.push(json!({
                            "role": "user",
                            "content": VALIDATION_FAILURE_RECOVERY_INSTRUCTION,
                        }));
                    }
                    if should_finish_after_dispatch_tasks(
                        dispatched_tasks.as_deref(),
                        &validation,
                        edit_recovery_required,
                    ) {
                        let content = dispatched_tasks
                            .as_deref()
                            .map(dispatched_tasks_completion_content)
                            .unwrap_or_default();
                        return Ok(ChatCompletionResponse {
                            content,
                            trace_steps,
                            usage: last_usage,
                            dispatched_tasks: dispatched_tasks.take(),
                        });
                    }
                    if edit_recovery_required || validation.requires_repair() {
                        final_answer_requested = false;
                    }
                    code_tool_called = true;
                    unproductive_turns = 0;
                    agent_turn_state.mark_tools_executed();
                    total_tool_calls_executed += tool_execution_plan.executable_count;
                    ToolCallUtils::schedule_checkpoint(
                        &mut tool_calls_since_checkpoint,
                        &mut tool_checkpoint_pending,
                        tool_execution_plan.executable_count,
                        TOOL_CALL_CHECKPOINT_INTERVAL,
                    );
                    if is_deepseek && tool_execution_plan.truncated {
                        tool_checkpoint_pending = true;
                    }
                    if total_tool_calls_executed >= MAX_TOTAL_TOOL_CALLS_PER_AUTONOMOUS_ROUND {
                        if ToolCallUtils::can_open_next_autonomous_round(
                            turn_index,
                            max_chat_completion_turns,
                        ) {
                            tool_checkpoint_pending = true;
                            tool_budget_reset_pending = true;
                            final_answer_requested = false;
                        } else {
                            tool_checkpoint_pending = false;
                            final_answer_requested = true;
                        }
                    }

                    if !validation_tool_required && !repair_required {
                        tool_only_rounds += 1;
                        if !tool_checkpoint_pending && tool_only_rounds >= MAX_TOOL_ONLY_ROUNDS {
                            if ToolCallUtils::can_open_next_autonomous_round(
                                turn_index,
                                max_chat_completion_turns,
                            ) {
                                tool_checkpoint_pending = true;
                                tool_budget_reset_pending = true;
                                final_answer_requested = false;
                            } else {
                                messages.push(json!({
                                    "role": "user",
                                    "content": FINAL_ANSWER_INSTRUCTION,
                                }));
                                final_answer_requested = true;
                            }
                        }
                    }

                    if validation.should_auto_validate(edit_recovery_required) {
                        validation.mark_auto_attempted();
                        let validation_run = code_workspace
                            .as_ref()
                            .map(|workspace| {
                                ValidationOps::run_default(
                                    &app,
                                    stream_id.as_deref(),
                                    workspace,
                                    can_write,
                                    &mut messages,
                                    &mut trace_steps,
                                )
                            })
                            .unwrap_or_default();

                        if !validation_run.ran() {
                            ValidationOps::mark_unavailable(&mut messages);
                            validation.mark_validator_discovery_required();
                            final_answer_requested = false;
                        } else if validation.record_run(validation_run) {
                            messages.push(json!({
                                "role": "user",
                                "content": VALIDATION_FAILURE_RECOVERY_INSTRUCTION,
                            }));
                            final_answer_requested = false;
                        } else {
                            final_answer_requested = false;
                        }
                    }
                    continue;
                }
            }
        }

        let content = StrUtils::message_content_text(&message);
        let reasoning = TraceCtx::message_reasoning(&message);

        let has_visible_output = !content.is_empty() || !reasoning.is_empty();

        if has_visible_output {
            if turn_phase == AgentTurnPhase::ToolReflection {
                messages.push(ToolCallUtils::trim_chat_tool_calls(&message, 0));
                agent_turn_state.complete_reflection();
                tool_checkpoint_pending = false;
                if tool_budget_reset_pending {
                    tool_only_rounds = 0;
                    tool_calls_since_checkpoint = 0;
                    total_tool_calls_executed = 0;
                    tool_budget_reset_pending = false;
                }

                // When content is empty but reasoning_content exists (DeepSeek thinking-only
                // reflection), the effective reflection decision can be extracted from a fallback
                // source since the model's decision is embedded in the thinking tokens.
                let effective_content = if !content.is_empty() {
                    &content
                } else {
                    &reasoning
                };
                match AgentTurnState::parse_reflection(effective_content) {
                    AgentReflectionDecision::Continue => {
                        // After repeated undecided reflections, ask for the final
                        // answer directly instead of looping action/reflection.
                        final_answer_requested = agent_turn_state.record_unresolved_reflection();
                    }
                    AgentReflectionDecision::Finish(content) => {
                        if finish_reason_indicates_truncated_output(last_finish_reason.as_deref()) {
                            append_continued_output(&mut accumulated_display_content, &content);
                            messages.push(json!({
                                "role": "user",
                                "content": CONTINUE_OUTPUT_INSTRUCTION,
                            }));
                            final_answer_requested = true;
                            continue;
                        }
                        return Ok(ChatCompletionResponse {
                            content: combined_output_text(&accumulated_display_content, &content),
                            trace_steps,
                            usage: last_usage,
                            dispatched_tasks: dispatched_tasks.take(),
                        });
                    }
                    AgentReflectionDecision::RequestFinalAnswer => {
                        final_answer_requested = true;
                    }
                }
                continue;
            }
            if turn_phase == AgentTurnPhase::BudgetCheckpoint {
                messages.push(ToolCallUtils::trim_chat_tool_calls(&message, 0));
                tool_checkpoint_pending = false;
                final_answer_requested = false;
                if tool_budget_reset_pending {
                    tool_only_rounds = 0;
                    tool_calls_since_checkpoint = 0;
                    total_tool_calls_executed = 0;
                    tool_budget_reset_pending = false;
                }
                continue;
            }
            if validation.is_pending() {
                if validation.can_auto_validate() {
                    validation.mark_auto_attempted();
                    let validation_run = code_workspace
                        .as_ref()
                        .map(|workspace| {
                            ValidationOps::run_default(
                                &app,
                                stream_id.as_deref(),
                                workspace,
                                can_write,
                                &mut messages,
                                &mut trace_steps,
                            )
                        })
                        .unwrap_or_default();

                    if validation_run.ran() {
                        if validation.record_run(validation_run) {
                            messages.push(json!({
                                "role": "user",
                                "content": VALIDATION_FAILURE_RECOVERY_INSTRUCTION,
                            }));
                            final_answer_requested = false;
                        } else {
                            final_answer_requested = false;
                        }
                        continue;
                    } else {
                        ValidationOps::mark_unavailable(&mut messages);
                        validation.mark_validator_discovery_required();
                        final_answer_requested = false;
                        continue;
                    }
                }
                final_answer_requested = false;
                continue;
            }
            if turn_phase == AgentTurnPhase::ToolAction && deepseek_tool_workflow {
                // The model declined to call a tool on an action turn — let it
                // stop on its own instead of forcing another tool round.
                messages.push(ToolCallUtils::trim_chat_tool_calls(&message, 0));
                if !content.is_empty() {
                    let action_answer = match AgentTurnState::parse_reflection(&content) {
                        AgentReflectionDecision::Finish(answer) => answer,
                        AgentReflectionDecision::RequestFinalAnswer => {
                            final_answer_requested = true;
                            continue;
                        }
                        // Plain prose without a control line is also a stop
                        // decision: accept it as the final answer.
                        AgentReflectionDecision::Continue => content.clone(),
                    };
                    if finish_reason_indicates_truncated_output(last_finish_reason.as_deref()) {
                        append_continued_output(&mut accumulated_display_content, &action_answer);
                        messages.push(json!({
                            "role": "user",
                            "content": CONTINUE_OUTPUT_INSTRUCTION,
                        }));
                        final_answer_requested = true;
                        continue;
                    }
                    return Ok(ChatCompletionResponse {
                        content: combined_output_text(&accumulated_display_content, &action_answer),
                        trace_steps,
                        usage: last_usage,
                        dispatched_tasks: dispatched_tasks.take(),
                    });
                }
                // Thinking-only output with no tool call: ask for the answer
                // directly instead of spinning on more action turns.
                final_answer_requested = true;
                continue;
            }

            if finish_reason_indicates_truncated_output(last_finish_reason.as_deref()) {
                append_continued_output(&mut accumulated_display_content, &content);
                messages.push(message.clone());
                messages.push(json!({
                    "role": "user",
                    "content": CONTINUE_OUTPUT_INSTRUCTION,
                }));
                final_answer_requested = true;
                continue;
            }

            // Hard stall termination: a turn with no tool calls and no usable
            // content must not return an empty answer, and must not loop
            // forever either. Force a couple of retries, then end the
            // generation with whatever was accumulated plus a note.
            if content.trim().is_empty() {
                unproductive_turns += 1;
                if unproductive_turns < MAX_UNPRODUCTIVE_TURNS {
                    messages.push(ToolCallUtils::trim_chat_tool_calls(&message, 0));
                    final_answer_requested = true;
                    continue;
                }

                let stalled_content = if accumulated_display_content.trim().is_empty() {
                    "Generation stalled: the model produced no usable content after several retries. Please try again or rephrase the request.".to_string()
                } else {
                    format!(
                        "{}\n\n(Generation ended early: the model stalled without further output.)",
                        accumulated_display_content.trim_end()
                    )
                };
                return Ok(ChatCompletionResponse {
                    content: stalled_content,
                    trace_steps,
                    usage: last_usage,
                    dispatched_tasks: dispatched_tasks.take(),
                });
            }

            // The model announced more work ("让我继续…", "Let me …", "待确认: …")
            // but called no tools. Accepting that as the final answer cuts the
            // task off mid-intent: push it to follow through (bounded by the
            // same stall counter, so it cannot loop forever).
            if content_signals_continuation_intent(&content)
                && unproductive_turns + 1 < MAX_UNPRODUCTIVE_TURNS
            {
                unproductive_turns += 1;
                emit_instruction_step(
                    &app,
                    stream_id.as_deref(),
                    &mut trace_steps,
                    "follow-through",
                    FOLLOW_THROUGH_INSTRUCTION,
                );
                messages.push(message.clone());
                messages.push(json!({
                    "role": "user",
                    "content": FOLLOW_THROUGH_INSTRUCTION,
                }));
                final_answer_requested = true;
                continue;
            }

            return Ok(ChatCompletionResponse {
                content: combined_output_text(&accumulated_display_content, &content),
                trace_steps,
                usage: last_usage,
                dispatched_tasks: dispatched_tasks.take(),
            });
        }

        if turn_phase == AgentTurnPhase::ToolReflection {
            // Store the message even when content is empty while reasoning_content is present,
            // so DeepSeek's thinking-mode context is preserved across turns.
            if !reasoning.is_empty() {
                messages.push(ToolCallUtils::trim_chat_tool_calls(&message, 0));
                match AgentTurnState::parse_reflection(&reasoning) {
                    AgentReflectionDecision::Continue => {
                        // After repeated undecided reflections, ask for the final
                        // answer directly instead of looping action/reflection.
                        final_answer_requested = agent_turn_state.record_unresolved_reflection();
                    }
                    AgentReflectionDecision::Finish(finish_content) => {
                        if finish_reason_indicates_truncated_output(last_finish_reason.as_deref()) {
                            append_continued_output(
                                &mut accumulated_display_content,
                                &finish_content,
                            );
                            messages.push(json!({
                                "role": "user",
                                "content": CONTINUE_OUTPUT_INSTRUCTION,
                            }));
                            final_answer_requested = true;
                        } else {
                            return Ok(ChatCompletionResponse {
                                content: combined_output_text(
                                    &accumulated_display_content,
                                    &finish_content,
                                ),
                                trace_steps,
                                usage: last_usage,
                                dispatched_tasks: dispatched_tasks.take(),
                            });
                        }
                    }
                    AgentReflectionDecision::RequestFinalAnswer => {
                        final_answer_requested = true;
                    }
                }
                agent_turn_state.complete_reflection();
                tool_checkpoint_pending = false;
                if tool_budget_reset_pending {
                    tool_only_rounds = 0;
                    tool_calls_since_checkpoint = 0;
                    total_tool_calls_executed = 0;
                    tool_budget_reset_pending = false;
                }
                continue;
            }
            agent_turn_state.complete_reflection();
            tool_checkpoint_pending = false;
            final_answer_requested = false;
            if tool_budget_reset_pending {
                tool_only_rounds = 0;
                tool_calls_since_checkpoint = 0;
                total_tool_calls_executed = 0;
                tool_budget_reset_pending = false;
            }
            continue;
        }

        if turn_phase == AgentTurnPhase::BudgetCheckpoint {
            tool_checkpoint_pending = false;
            final_answer_requested = false;
            if tool_budget_reset_pending {
                tool_only_rounds = 0;
                tool_calls_since_checkpoint = 0;
                total_tool_calls_executed = 0;
                tool_budget_reset_pending = false;
            }
            continue;
        }

        if turn_phase == AgentTurnPhase::ToolAction && deepseek_tool_workflow {
            // Empty action-turn output: ask for the answer directly instead of
            // spinning on more action turns.
            final_answer_requested = true;
            continue;
        }

        if code_tool_called && !final_answer_requested {
            final_answer_requested = true;
            continue;
        }
    }

    let reason = last_finish_reason
        .map(|value| format!(" finish_reason={}", value))
        .unwrap_or_default();
    Err(format!(
        "{} returned no displayable content.{}",
        request.provider_name, reason
    ))
}

#[tauri::command(rename_all = "camelCase")]
pub fn cancel_chat_completion(
    cancellation_id: String,
    cancellation_state: State<'_, ChatCancellationState>,
) {
    cancellation_state.cancel(cancellation_id.trim());
}

#[tauri::command(rename_all = "camelCase")]
pub fn finish_chat_completion(
    cancellation_id: String,
    cancellation_state: State<'_, ChatCancellationState>,
) {
    cancellation_state.finish(cancellation_id.trim());
}

pub(crate) async fn openai_responses_completion(
    app: AppHandle,
    request: ChatCompletionRequest,
    code_workspace: Option<PathBuf>,
    cancellation: Option<Arc<AtomicBool>>,
) -> Result<ChatCompletionResponse, String> {
    let endpoint = responses_endpoint(&request.base_url);
    let client = reqwest::Client::new();
    let can_write = request.can_write.unwrap_or(false);
    let is_deepseek = is_deepseek_provider(&request.provider_name, &request.base_url);
    let max_tool_calls_per_turn = if is_deepseek {
        MAX_DEEPSEEK_TOOL_CALLS_PER_TURN
    } else {
        TOOL_CALL_CHECKPOINT_INTERVAL
    };
    let mut code_tool_called = false;
    let mut edit_recovery_required = false;
    let mut edit_recovery_rounds = 0usize;
    let mut final_answer_requested = false;
    let mut tool_only_rounds: usize = 0;
    let mut tool_calls_since_checkpoint = 0usize;
    let mut total_tool_calls_executed = 0usize;
    let mut tool_checkpoint_pending = false;
    let mut tool_budget_reset_pending = false;
    let mut validation = ValidationState::default();
    let mut no_progress_guard = NoProgressGuard::default();
    let mut previous_response_id: Option<String> = None;
    let mut pending_input: Vec<Value> = Vec::new();
    let mut last_usage: Option<ChatCompletionUsage> = None;
    let mut trace_steps: Vec<ChatTraceStep> = Vec::new();
    let mut last_turn_instruction: Option<&'static str> = None;
    let mut accumulated_display_content = String::new();
    let stream_id = request
        .stream_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);

    for turn_index in 0..MAX_CHAT_COMPLETION_TURNS {
        let validation_tool_required = validation.requires_tool(edit_recovery_required);
        if validation_tool_required {
            validation.mark_model_prompted();
        }
        let repair_required = edit_recovery_required || validation.requires_repair();
        let checkpoint_required =
            tool_checkpoint_pending && !validation_tool_required && !repair_required;

        let mut input = if previous_response_id.is_some() {
            std::mem::take(&mut pending_input)
        } else {
            responses_payload_messages(
                &request.messages,
                final_answer_requested && !validation_tool_required && !repair_required,
                validation_tool_required,
                checkpoint_required,
            )
        };

        if previous_response_id.is_some() {
            if validation_tool_required {
                input.push(responses_user_message(VALIDATION_REQUIRED_INSTRUCTION));
            } else if checkpoint_required {
                input.push(responses_user_message(TOOL_CALL_CHECKPOINT_INSTRUCTION));
            } else if final_answer_requested && !repair_required {
                input.push(responses_user_message(FINAL_ANSWER_INSTRUCTION));
            }
        }

        let turn_instruction = if validation_tool_required {
            Some(("validation-required", VALIDATION_REQUIRED_INSTRUCTION))
        } else if checkpoint_required {
            Some(("checkpoint", TOOL_CALL_CHECKPOINT_INSTRUCTION))
        } else if final_answer_requested && !validation_tool_required && !repair_required {
            Some(("final-answer", FINAL_ANSWER_INSTRUCTION))
        } else {
            None
        };
        // Surface each newly injected instruction, skipping consecutive repeats.
        if let Some((label, instruction)) = turn_instruction {
            if last_turn_instruction != Some(instruction) {
                emit_instruction_step(&app, stream_id.as_deref(), &mut trace_steps, label, instruction);
                last_turn_instruction = Some(instruction);
            }
        } else {
            last_turn_instruction = None;
        }

        let code_tools_allowed = !checkpoint_required
            && code_workspace.is_some()
            && (!final_answer_requested || validation_tool_required || repair_required);
        let orchestration_tools_allowed = !checkpoint_required
            && !final_answer_requested
            && request.orchestration_tools_enabled.unwrap_or(false);
        let orchestration_required = request.orchestration_required.unwrap_or(false);
        let any_tools_allowed = code_tools_allowed || orchestration_tools_allowed;
        let mut payload = json!({
            "model": request.model,
            "input": input,
        });

        if let Some(system_prompt) = request.system_prompt.as_deref() {
            let trimmed = system_prompt.trim();
            if !trimmed.is_empty() {
                payload["instructions"] = json!(trimmed);
            }
        }

        if let Some(previous_response_id) = previous_response_id.as_deref() {
            payload["previous_response_id"] = json!(previous_response_id);
        }

        let reasoning_effort = if checkpoint_required {
            Some(TOOL_CALL_CHECKPOINT_REASONING_EFFORT)
        } else {
            request.reasoning_effort.as_deref()
        };

        if let Some(reasoning) = responses_reasoning_payload(reasoning_effort) {
            payload["reasoning"] = reasoning;
        }

        if any_tools_allowed {
            let mut tools: Vec<Value> = Vec::new();
            if code_tools_allowed {
                if let Value::Array(code_tools) = responses_tools_schema(can_write) {
                    tools.extend(code_tools);
                }
            }
            if orchestration_tools_allowed {
                if let Value::Array(orchestration_tools) = responses_orchestration_tools_schema() {
                    tools.extend(orchestration_tools);
                }
            }
            // No-progress escalation: remove read/explore tools so the model
            // must edit, run a command, or answer instead of stalling.
            if no_progress_guard.read_tools_blocked() {
                retain_non_read_tools(&mut tools);
            }
            if !tools.is_empty() {
                payload["tools"] = Value::Array(tools);
                if repair_required || orchestration_required {
                    payload["tool_choice"] = json!("required");
                }
            }
        }

        let parsed = send_responses_request_maybe_stream(
            &app,
            stream_id.as_deref(),
            &client,
            &endpoint,
            &request.api_key,
            &request.provider_name,
            &payload,
            cancellation.as_deref(),
        )
        .await?;

        if let Some(response_id) = responses_id(&parsed) {
            previous_response_id = Some(response_id);
        }

        if let Some(usage) = TraceCtx::usage_from(&parsed) {
            last_usage = Some(usage);
        }

        TraceCtx::append_steps(&mut trace_steps, responses_reasoning_trace_steps(&parsed));
        let tool_calls = responses_function_calls(&parsed);

        if !checkpoint_required {
            if let Some(workspace) = code_workspace.as_ref() {
                if !tool_calls.is_empty() {
                    let remaining_total_tool_budget = MAX_TOTAL_TOOL_CALLS_PER_AUTONOMOUS_ROUND
                        .saturating_sub(total_tool_calls_executed);
                    // Apply the same hard batch cap for Responses API function calls.
                    let tool_execution_plan = ToolCallUtils::plan_execution(
                        tool_calls.len(),
                        tool_calls_since_checkpoint,
                        TOOL_CALL_CHECKPOINT_INTERVAL,
                        max_tool_calls_per_turn,
                        remaining_total_tool_budget,
                    );
                    if tool_execution_plan.executable_count == 0 {
                        if ToolCallUtils::can_open_next_autonomous_round(
                            turn_index,
                            MAX_CHAT_COMPLETION_TURNS,
                        ) {
                            tool_checkpoint_pending = true;
                            tool_budget_reset_pending = true;
                            final_answer_requested = false;
                        } else {
                            tool_checkpoint_pending = false;
                            final_answer_requested = true;
                        }
                        continue;
                    }
                    let executable_tool_calls = ToolCallUtils::truncate_tool_calls(
                        &tool_calls,
                        tool_execution_plan.executable_count,
                    );
                    let mut failed_edit = false;
                    let mut successful_edit = false;
                    let mut read_loop_nudge: Option<String> = None;
                    let mut validation_run = ValidationRun::default();
                    for response_tool_call in &executable_tool_calls {
                        let tool_call =
                            response_function_call_to_chat_tool_call(response_tool_call);
                        let call_step = tool_call_trace_step(&tool_call);
                        emit_trace_step(&app, stream_id.as_deref(), &call_step);
                        TraceCtx::append_steps(&mut trace_steps, vec![call_step]);

                        let tool_result = if let Some(tool_result) =
                            validation.redundant_validation_tool_result(&tool_call)
                        {
                            tool_result
                        } else {
                            let mut stream_tool_output = |step: ChatTraceStep| {
                                emit_tool_chunk(&app, stream_id.as_deref(), &step);
                            };
                            execute_code_tool_call(
                                workspace,
                                &tool_call,
                                can_write,
                                Some(&mut stream_tool_output),
                            )
                        };

                        validation_run.observe_tool_result(&tool_call, &tool_result);
                        if let Some(nudge) = no_progress_guard.record_tool_call(&tool_call) {
                            read_loop_nudge = Some(nudge);
                        }
                        if ValidationOps::result_succeeded(&tool_result)
                            && ValidationOps::is_edit_call(&tool_call)
                        {
                            successful_edit = true;
                        }
                        if ValidationOps::edit_needs_recovery(&tool_call, &tool_result) {
                            failed_edit = true;
                        }

                        let result_step = tool_result_trace_step(&tool_call, &tool_result);
                        emit_trace_step(&app, stream_id.as_deref(), &result_step);
                        TraceCtx::append_steps(&mut trace_steps, vec![result_step]);
                        pending_input.push(response_tool_output(&tool_call, &tool_result));
                    }

                    (edit_recovery_required, edit_recovery_rounds) =
                        ValidationOps::next_recovery_state(
                            edit_recovery_required,
                            edit_recovery_rounds,
                            failed_edit,
                            successful_edit,
                        );
                    if failed_edit && edit_recovery_required {
                        emit_instruction_step(&app, stream_id.as_deref(), &mut trace_steps, "edit-recovery", EDIT_FAILURE_RECOVERY_INSTRUCTION);
                        pending_input
                            .push(responses_user_message(EDIT_FAILURE_RECOVERY_INSTRUCTION));
                    }
                    if let Some(nudge) = read_loop_nudge {
                        emit_instruction_step(&app, stream_id.as_deref(), &mut trace_steps, "no-progress-nudge", &nudge);
                        pending_input.push(responses_user_message(nudge.as_str()));
                    }
                    let validation_failed = if successful_edit {
                        validation.mark_successful_edit();
                        false
                    } else {
                        validation.record_run(validation_run)
                    };
                    if validation_failed {
                        emit_instruction_step(&app, stream_id.as_deref(), &mut trace_steps, "validation-failure", VALIDATION_FAILURE_RECOVERY_INSTRUCTION);
                        pending_input.push(responses_user_message(
                            VALIDATION_FAILURE_RECOVERY_INSTRUCTION,
                        ));
                    }
                    if edit_recovery_required || validation.requires_repair() {
                        final_answer_requested = false;
                    }

                    code_tool_called = true;
                    total_tool_calls_executed += executable_tool_calls.len();
                    ToolCallUtils::schedule_checkpoint(
                        &mut tool_calls_since_checkpoint,
                        &mut tool_checkpoint_pending,
                        executable_tool_calls.len(),
                        TOOL_CALL_CHECKPOINT_INTERVAL,
                    );
                    if is_deepseek && tool_execution_plan.truncated {
                        tool_checkpoint_pending = true;
                    }
                    if total_tool_calls_executed >= MAX_TOTAL_TOOL_CALLS_PER_AUTONOMOUS_ROUND {
                        if ToolCallUtils::can_open_next_autonomous_round(
                            turn_index,
                            MAX_CHAT_COMPLETION_TURNS,
                        ) {
                            tool_checkpoint_pending = true;
                            tool_budget_reset_pending = true;
                            final_answer_requested = false;
                        } else {
                            tool_checkpoint_pending = false;
                            final_answer_requested = true;
                        }
                    }

                    if !validation_tool_required && !repair_required {
                        tool_only_rounds += 1;
                        if !tool_checkpoint_pending && tool_only_rounds >= MAX_TOOL_ONLY_ROUNDS {
                            if ToolCallUtils::can_open_next_autonomous_round(
                                turn_index,
                                MAX_CHAT_COMPLETION_TURNS,
                            ) {
                                tool_checkpoint_pending = true;
                                tool_budget_reset_pending = true;
                                final_answer_requested = false;
                            } else {
                                emit_instruction_step(&app, stream_id.as_deref(), &mut trace_steps, "final-answer-forced", FINAL_ANSWER_INSTRUCTION);
                                pending_input
                                    .push(responses_user_message(FINAL_ANSWER_INSTRUCTION));
                                final_answer_requested = true;
                            }
                        }
                    }

                    if validation.should_auto_validate(edit_recovery_required) {
                        validation.mark_auto_attempted();
                        let (validation_outputs, validation_run) =
                            run_default_validation_commands_for_responses(
                                &app,
                                stream_id.as_deref(),
                                workspace,
                                can_write,
                                &mut trace_steps,
                            );

                        if validation_outputs.is_empty() {
                            emit_instruction_step(&app, stream_id.as_deref(), &mut trace_steps, "validation-unavailable", VALIDATION_UNAVAILABLE_INSTRUCTION);
                            mark_validation_unavailable_for_responses(&mut pending_input);
                            validation.mark_validator_discovery_required();
                            final_answer_requested = false;
                        } else {
                            pending_input.extend(validation_outputs);
                            if validation.record_run(validation_run) {
                                pending_input.push(responses_user_message(
                                    VALIDATION_FAILURE_RECOVERY_INSTRUCTION,
                                ));
                                final_answer_requested = false;
                            } else {
                                final_answer_requested = false;
                            }
                        }
                    }

                    continue;
                }
            }
        }

        let content = responses_output_text(&parsed);

        if !content.is_empty() {
            if checkpoint_required {
                tool_checkpoint_pending = false;
                final_answer_requested = false;
                if tool_budget_reset_pending {
                    tool_only_rounds = 0;
                    tool_calls_since_checkpoint = 0;
                    total_tool_calls_executed = 0;
                    tool_budget_reset_pending = false;
                }
                continue;
            }
            if validation.is_pending() {
                if let Some(workspace) = code_workspace.as_ref() {
                    if validation.can_auto_validate() {
                        validation.mark_auto_attempted();
                        let (validation_outputs, validation_run) =
                            run_default_validation_commands_for_responses(
                                &app,
                                stream_id.as_deref(),
                                workspace,
                                can_write,
                                &mut trace_steps,
                            );

                        if !validation_outputs.is_empty() {
                            pending_input.extend(validation_outputs);
                            if validation.record_run(validation_run) {
                                pending_input.push(responses_user_message(
                                    VALIDATION_FAILURE_RECOVERY_INSTRUCTION,
                                ));
                                final_answer_requested = false;
                            } else {
                                final_answer_requested = false;
                            }
                            continue;
                        }
                    }
                }

                emit_instruction_step(&app, stream_id.as_deref(), &mut trace_steps, "validation-unavailable", VALIDATION_UNAVAILABLE_INSTRUCTION);
                mark_validation_unavailable_for_responses(&mut pending_input);
                validation.mark_validator_discovery_required();
                final_answer_requested = false;
                continue;
            }

            if responses_output_is_incomplete(&parsed) {
                append_continued_output(&mut accumulated_display_content, &content);
                emit_instruction_step(&app, stream_id.as_deref(), &mut trace_steps, "continue-output", CONTINUE_OUTPUT_INSTRUCTION);
                pending_input.push(responses_user_message(CONTINUE_OUTPUT_INSTRUCTION));
                final_answer_requested = true;
                continue;
            }

            return Ok(ChatCompletionResponse {
                content: combined_output_text(&accumulated_display_content, &content),
                trace_steps,
                usage: last_usage,
                dispatched_tasks: None,
            });
        }

        if checkpoint_required {
            tool_checkpoint_pending = false;
            final_answer_requested = false;
            if tool_budget_reset_pending {
                tool_only_rounds = 0;
                tool_calls_since_checkpoint = 0;
                total_tool_calls_executed = 0;
                tool_budget_reset_pending = false;
            }
            continue;
        }

        if code_tool_called && !final_answer_requested {
            final_answer_requested = true;
            continue;
        }
    }

    Err(format!(
        "{} returned no displayable content from Responses API.",
        request.provider_name
    ))
}

/// Detects replies that end by announcing more work ("让我把分析讲完…",
/// "Let me check…", "待确认: …") instead of doing it. Accepting those as the
/// final answer cuts the task off mid-intent, so the loop treats them as
/// unfinished and pushes the model to follow through.
pub(crate) fn content_signals_continuation_intent(content: &str) -> bool {
    let tail: String = content
        .trim_end()
        .chars()
        .rev()
        .take(240)
        .collect::<String>()
        .chars()
        .rev()
        .collect();

    const ZH_PENDING_MARKERS: &[&str] = &["待确认", "待办", "待完成", "待处理"];
    if ZH_PENDING_MARKERS
        .iter()
        .any(|marker| tail.contains(marker))
    {
        return true;
    }

    let Some(last_sentence) = tail
        .split(|ch| matches!(ch, '。' | '！' | '？' | '.' | '!' | '?' | '\n'))
        .map(str::trim)
        .filter(|sentence| !sentence.is_empty())
        .last()
    else {
        return false;
    };

    const ZH_INTENT_MARKERS: &[&str] = &[
        "让我", "我先", "我继续", "我接下来", "接下来我", "下面我", "我现在", "我马上", "我这就",
    ];
    const EN_INTENT_MARKERS: &[&str] = &[
        "let me ",
        "i'll ",
        "i will ",
        "now i'll",
        "next i'll",
        "next i will",
    ];

    last_sentence
        .split(|ch| matches!(ch, '，' | '—' | ',' | ';' | '；' | '：' | ':'))
        .map(|clause| {
            clause
                .trim()
                .trim_start_matches(['"', '“', '「', '*', '`', '#', '>', '-', ' '])
        })
        .filter(|clause| !clause.is_empty())
        .any(|clause| {
            ZH_INTENT_MARKERS
                .iter()
                .any(|marker| clause.starts_with(marker))
                || EN_INTENT_MARKERS
                    .iter()
                    .any(|marker| clause.to_ascii_lowercase().starts_with(marker))
        })
}

/// Port of Kun's model-history repair: the payload sent upstream must be
/// structurally legal no matter which loop path produced the history.
/// Consecutive assistant tool_call messages (one model response split by
/// loop bookkeeping) merge back into a single legal tool_calls message;
/// orphan `tool` results (no matching assistant tool_call) are dropped; and
/// assistant tool_calls without a following `tool` result are removed (the
/// message itself is dropped when nothing else remains).
pub(crate) fn repair_model_history(messages: &mut Vec<Value>) {
    // Pass 1: merge consecutive assistant tool_call messages.
    let mut merged: Vec<Value> = Vec::with_capacity(messages.len());

    for message in messages.drain(..) {
        let is_assistant_with_tool_calls = message.get("role").and_then(Value::as_str)
            == Some("assistant")
            && message
                .get("tool_calls")
                .and_then(Value::as_array)
                .is_some_and(|tool_calls| !tool_calls.is_empty());

        if is_assistant_with_tool_calls {
            if let Some(previous) = merged.last_mut() {
                let previous_is_assistant_with_tool_calls =
                    previous.get("role").and_then(Value::as_str) == Some("assistant")
                        && previous
                            .get("tool_calls")
                            .and_then(Value::as_array)
                            .is_some_and(|tool_calls| !tool_calls.is_empty());

                if previous_is_assistant_with_tool_calls {
                    let mut previous_tool_calls = previous
                        .get("tool_calls")
                        .and_then(Value::as_array)
                        .cloned()
                        .unwrap_or_default();
                    let mut next_tool_calls = message
                        .get("tool_calls")
                        .and_then(Value::as_array)
                        .cloned()
                        .unwrap_or_default();
                    previous_tool_calls.append(&mut next_tool_calls);
                    previous["tool_calls"] = Value::Array(previous_tool_calls);

                    let next_content = StrUtils::message_content_text(&message);
                    if !next_content.is_empty() {
                        let previous_content = StrUtils::message_content_text(previous);
                        previous["content"] = json!(if previous_content.is_empty() {
                            next_content
                        } else {
                            format!("{previous_content}\n{next_content}")
                        });
                    }

                    if previous.get("reasoning_content").is_none() {
                        if let Some(reasoning) = message.get("reasoning_content").cloned() {
                            previous["reasoning_content"] = reasoning;
                        }
                    }
                    continue;
                }
            }
        }

        merged.push(message);
    }

    // Pass 2: drop orphan tool results and unanswered tool calls.
    let mut call_ids = std::collections::HashSet::new();
    for message in &merged {
        if message.get("role").and_then(Value::as_str) != Some("assistant") {
            continue;
        }
        if let Some(tool_calls) = message.get("tool_calls").and_then(Value::as_array) {
            for tool_call in tool_calls {
                if let Some(id) = tool_call.get("id").and_then(Value::as_str) {
                    call_ids.insert(id.to_string());
                }
            }
        }
    }

    let mut result_ids = std::collections::HashSet::new();
    for message in &merged {
        if message.get("role").and_then(Value::as_str) != Some("tool") {
            continue;
        }
        if let Some(id) = message.get("tool_call_id").and_then(Value::as_str) {
            result_ids.insert(id.to_string());
        }
    }

    merged.retain_mut(|message| {
        let role = message.get("role").and_then(Value::as_str).unwrap_or_default();

        if role == "tool" {
            let id = message
                .get("tool_call_id")
                .and_then(Value::as_str)
                .unwrap_or_default();
            return call_ids.contains(id);
        }

        if role == "assistant" {
            if let Some(tool_calls) = message.get("tool_calls").and_then(Value::as_array) {
                let answered: Vec<Value> = tool_calls
                    .iter()
                    .filter(|tool_call| {
                        tool_call
                            .get("id")
                            .and_then(Value::as_str)
                            .is_some_and(|id| result_ids.contains(id))
                    })
                    .cloned()
                    .collect();

                if answered.len() != tool_calls.len() {
                    if answered.is_empty() {
                        if let Some(object) = message.as_object_mut() {
                            object.remove("tool_calls");
                        }
                    } else {
                        message["tool_calls"] = Value::Array(answered);
                    }
                }
            }

            let has_tool_calls = message
                .get("tool_calls")
                .and_then(Value::as_array)
                .is_some_and(|tool_calls| !tool_calls.is_empty());
            let has_content = !StrUtils::message_content_text(message).is_empty();
            let has_reasoning = message
                .get("reasoning_content")
                .and_then(Value::as_str)
                .is_some_and(|reasoning| !reasoning.trim().is_empty());
            return has_tool_calls || has_content || has_reasoning;
        }

        true
    });

    *messages = merged;
}

pub(crate) fn is_deepseek_provider(provider_name: &str, base_url: &str) -> bool {
    provider_name.to_ascii_lowercase().contains("deepseek")
        || base_url.to_ascii_lowercase().contains("deepseek")
}

pub(crate) fn reasoning_enabled(reasoning_effort: Option<&str>) -> bool {
    let trimmed = reasoning_effort.unwrap_or("").trim();
    !trimmed.is_empty() && !trimmed.eq_ignore_ascii_case("off")
}

pub(crate) fn apply_reasoning_payload(payload: &mut Value, is_deepseek: bool, reasoning_effort: Option<&str>) {
    let trimmed = reasoning_effort.unwrap_or("").trim();
    let reasoning_enabled = reasoning_enabled(reasoning_effort);

    if is_deepseek {
        payload["thinking"] = if reasoning_enabled {
            json!({ "type": "enabled" })
        } else {
            json!({ "type": "disabled" })
        };
    }

    if reasoning_enabled {
        payload["reasoning_effort"] = json!(trimmed);
    }
}

/// DeepSeek thinking mode requires every assistant message that made tool
/// calls to carry reasoning_content in all subsequent requests — otherwise
/// the API answers HTTP 400 "The `reasoning_content` in the thinking mode
/// must be passed back to the API". Streamed turns with genuinely empty
/// reasoning miss the field, so backfill a placeholder at the payload
/// boundary instead of letting one lossy turn poison the rest of the loop.
pub(crate) fn backfill_deepseek_reasoning(messages: &mut [Value]) {
    for message in messages.iter_mut() {
        if message.get("role").and_then(Value::as_str) != Some("assistant") {
            continue;
        }

        let has_tool_calls = message
            .get("tool_calls")
            .and_then(Value::as_array)
            .is_some_and(|tool_calls| !tool_calls.is_empty());

        if !has_tool_calls {
            continue;
        }

        let has_reasoning = message
            .get("reasoning_content")
            .and_then(Value::as_str)
            .is_some_and(|reasoning| !reasoning.trim().is_empty());

        if !has_reasoning {
            message["reasoning_content"] =
                json!("(reasoning content was not retained for this turn)");
        }
    }
}

pub(crate) fn chat_payload_messages(
    messages: &[Value],
    final_answer_requested: bool,
    validation_required: bool,
    phase_instruction: Option<&str>,
) -> Vec<Value> {
    let mut payload_messages = messages.to_vec();

    if validation_required {
        payload_messages.push(json!({
            "role": "user",
            "content": VALIDATION_REQUIRED_INSTRUCTION,
        }));
    } else if let Some(phase_instruction) = phase_instruction {
        payload_messages.push(json!({
            "role": "user",
            "content": phase_instruction,
        }));
    } else if final_answer_requested {
        payload_messages.push(json!({
            "role": "user",
            "content": FINAL_ANSWER_INSTRUCTION,
        }));
    }

    payload_messages
}

pub(crate) fn responses_user_message(content: &str) -> Value {
    json!({
        "role": "user",
        "content": content,
    })
}

pub(crate) fn mark_validation_unavailable_for_responses(input: &mut Vec<Value>) {
    input.push(responses_user_message(VALIDATION_UNAVAILABLE_INSTRUCTION));
}

pub(crate) fn responses_payload_messages(
    messages: &[ChatMessage],
    final_answer_requested: bool,
    validation_required: bool,
    checkpoint_required: bool,
) -> Vec<Value> {
    let mut payload_messages = messages
        .iter()
        .map(|message| {
            let mut msg = json!({
                "role": message.role,
                "content": message.content,
            });
            if let Some(reasoning) = &message.reasoning_content {
                if !reasoning.trim().is_empty() {
                    msg["reasoning_content"] = json!(reasoning);
                }
            }
            msg
        })
        .collect::<Vec<_>>();

    if validation_required {
        payload_messages.push(responses_user_message(VALIDATION_REQUIRED_INSTRUCTION));
    } else if checkpoint_required {
        payload_messages.push(responses_user_message(TOOL_CALL_CHECKPOINT_INSTRUCTION));
    } else if final_answer_requested {
        payload_messages.push(responses_user_message(FINAL_ANSWER_INSTRUCTION));
    }

    payload_messages
}

pub(crate) fn responses_reasoning_payload(reasoning_effort: Option<&str>) -> Option<Value> {
    if !reasoning_enabled(reasoning_effort) {
        return None;
    }

    Some(json!({
        "effort": reasoning_effort.unwrap_or("").trim(),
        "summary": "auto",
    }))
}

/// Drops read/explore tools from a tools payload while the no-progress guard
/// is escalated, so the model cannot keep exploring and must edit, run a
/// command, or answer. Handles both tool shapes: chat-completions entries
/// (`function.name`) and flattened responses entries (top-level `name`).
pub(crate) fn retain_non_read_tools(tools: &mut Vec<Value>) {
    tools.retain(|tool| {
        let name = tool
            .get("function")
            .and_then(|function| function.get("name"))
            .and_then(Value::as_str)
            .or_else(|| tool.get("name").and_then(Value::as_str))
            .unwrap_or_default();
        !NoProgressGuard::BLOCKABLE_READ_TOOLS.contains(&name)
    });
}

pub(crate) fn responses_tools_schema(allow_writes: bool) -> Value {
    let tools = code_tools_schema(true, allow_writes)
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|tool| {
            let function = tool.get("function")?.clone();
            let mut response_tool = function;
            response_tool["type"] = json!("function");
            Some(response_tool)
        })
        .collect::<Vec<_>>();

    Value::Array(tools)
}

pub(crate) fn responses_orchestration_tools_schema() -> Value {
    let tools = orchestration_tools_schema(true)
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|tool| {
            let function = tool.get("function")?.clone();
            let mut response_tool = function;
            response_tool["type"] = json!("function");
            Some(response_tool)
        })
        .collect::<Vec<_>>();

    Value::Array(tools)
}

pub(crate) fn responses_id(response: &Value) -> Option<String> {
    response
        .get("id")
        .and_then(Value::as_str)
        .map(str::to_string)
}

pub(crate) fn responses_output_text(response: &Value) -> String {
    if let Some(text) = response.get("output_text").and_then(Value::as_str) {
        if !text.trim().is_empty() {
            return text.trim().to_string();
        }
    }

    response
        .get("output")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|item| item.get("type").and_then(Value::as_str) == Some("message"))
        .flat_map(|item| {
            item.get("content")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
        })
        .filter_map(|part| {
            part.get("text")
                .or_else(|| part.get("content"))
                .and_then(Value::as_str)
        })
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

pub(crate) fn responses_reasoning_trace_steps(response: &Value) -> Vec<ChatTraceStep> {
    response
        .get("output")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|item| item.get("type").and_then(Value::as_str) == Some("reasoning"))
        .flat_map(|item| {
            item.get("summary")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
        })
        .filter_map(|summary| {
            summary
                .get("text")
                .or_else(|| summary.get("content"))
                .and_then(Value::as_str)
        })
        .flat_map(TraceCtx::split_trace)
        .map(|line| TraceCtx::trace_step("reasoning", line))
        .collect()
}

pub(crate) fn responses_function_calls(response: &Value) -> Vec<Value> {
    response
        .get("output")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|item| item.get("type").and_then(Value::as_str) == Some("function_call"))
        .cloned()
        .collect()
}

pub(crate) fn response_function_call_to_chat_tool_call(function_call: &Value) -> Value {
    let call_id = function_call
        .get("call_id")
        .or_else(|| function_call.get("id"))
        .and_then(Value::as_str)
        .unwrap_or("responses-tool-call");
    let name = function_call
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let arguments = function_call
        .get("arguments")
        .and_then(Value::as_str)
        .unwrap_or("{}");

    json!({
        "id": call_id,
        "type": "function",
        "function": {
            "name": name,
            "arguments": arguments,
        }
    })
}

pub(crate) fn chat_tool_call_to_response_function_call(tool_call: &Value) -> Value {
    let call_id = tool_call
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or("responses-tool-call");
    let function = tool_call.get("function").unwrap_or(&Value::Null);
    let name = function.get("name").and_then(Value::as_str).unwrap_or("");
    let arguments = function
        .get("arguments")
        .and_then(Value::as_str)
        .unwrap_or("{}");

    json!({
        "type": "function_call",
        "call_id": call_id,
        "name": name,
        "arguments": arguments,
    })
}

pub(crate) fn response_tool_output(tool_call: &Value, tool_result: &Value) -> Value {
    let call_id = tool_call
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or("responses-tool-call");

    json!({
        "type": "function_call_output",
        "call_id": call_id,
        "output": StrUtils::message_content_text(tool_result),
    })
}

pub(crate) fn run_default_validation_commands_for_responses(
    app: &AppHandle,
    stream_id: Option<&str>,
    workspace: &Path,
    can_write: bool,
    trace_steps: &mut Vec<ChatTraceStep>,
) -> (Vec<Value>, ValidationRun) {
    let mut outputs = Vec::new();
    let mut run = ValidationRun::default();

    for tool_call in ValidationOps::make_calls(workspace) {
        let call_step = tool_call_trace_step(&tool_call);
        emit_trace_step(app, stream_id, &call_step);
        TraceCtx::append_steps(trace_steps, vec![call_step]);

        let mut stream_tool_output = |step: ChatTraceStep| {
            emit_tool_chunk(app, stream_id, &step);
        };
        let tool_result = execute_code_tool_call(
            workspace,
            &tool_call,
            can_write,
            Some(&mut stream_tool_output),
        );
        let result_step = tool_result_trace_step(&tool_call, &tool_result);
        emit_trace_step(app, stream_id, &result_step);
        TraceCtx::append_steps(trace_steps, vec![result_step]);
        run.observe_tool_result(&tool_call, &tool_result);

        outputs.push(chat_tool_call_to_response_function_call(&tool_call));
        outputs.push(response_tool_output(&tool_call, &tool_result));
    }

    (outputs, run)
}

pub(crate) fn first_choice_finish_reason(parsed: &Value) -> Option<String> {
    parsed
        .get("choices")
        .and_then(|choices| choices.get(0))
        .and_then(|choice| choice.get("finish_reason"))
        .and_then(Value::as_str)
        .map(str::to_string)
}

pub(crate) fn finish_reason_indicates_truncated_output(reason: Option<&str>) -> bool {
    reason.is_some_and(|value| {
        matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "length" | "max_tokens"
        )
    })
}

pub(crate) fn responses_output_is_incomplete(response: &Value) -> bool {
    response
        .get("status")
        .and_then(Value::as_str)
        .is_some_and(|status| status.eq_ignore_ascii_case("incomplete"))
        || response.get("incomplete_details").is_some()
}

pub(crate) fn append_continued_output(accumulated: &mut String, next: &str) {
    let trimmed = next.trim();
    if trimmed.is_empty() {
        return;
    }

    if !accumulated.is_empty() {
        accumulated.push('\n');
        accumulated.push('\n');
    }

    accumulated.push_str(trimmed);
}

pub(crate) fn combined_output_text(accumulated: &str, next: &str) -> String {
    let trimmed_next = next.trim();
    if accumulated.is_empty() {
        return trimmed_next.to_string();
    }
    if trimmed_next.is_empty() {
        return accumulated.to_string();
    }

    format!("{}\n\n{}", accumulated, trimmed_next)
}

pub(crate) fn should_retry_http_failure(status: reqwest::StatusCode, _body: &str) -> bool {
    status.is_server_error()
        || matches!(
            status,
            reqwest::StatusCode::REQUEST_TIMEOUT
                | reqwest::StatusCode::TOO_MANY_REQUESTS
                | reqwest::StatusCode::CONFLICT
        )
}

pub(crate) fn request_was_cancelled(cancellation: Option<&AtomicBool>) -> bool {
    cancellation.is_some_and(|token| token.load(Ordering::Acquire))
}

pub(crate) async fn wait_for_http_retry(
    delay: Duration,
    cancellation: Option<&AtomicBool>,
) -> Result<(), String> {
    let deadline = Instant::now() + delay;

    loop {
        if request_was_cancelled(cancellation) {
            return Err("Chat completion was cancelled.".to_string());
        }

        let now = Instant::now();
        if now >= deadline {
            return Ok(());
        }

        tokio::time::sleep((deadline - now).min(RETRY_CANCELLATION_POLL_INTERVAL)).await;
    }
}

pub(crate) async fn send_http_request_with_retry(
    client: &reqwest::Client,
    endpoint: &str,
    api_key: &str,
    provider_name: &str,
    payload: &Value,
    cancellation: Option<&AtomicBool>,
    retry_progress: Option<&(dyn Fn(HttpRetryProgress) + Sync)>,
    initial_retry_delay: Duration,
) -> Result<reqwest::Response, String> {
    let retry_delay = initial_retry_delay;
    let mut retry_attempt = 0;

    loop {
        if request_was_cancelled(cancellation) {
            return Err("Chat completion was cancelled.".to_string());
        }

        let response = match client
            .post(endpoint)
            .bearer_auth(api_key.trim())
            .json(payload)
            .send()
            .await
        {
            Ok(response) => response,
            Err(error) => {
                retry_attempt += 1;
                if let Some(notify) = retry_progress {
                    notify(HttpRetryProgress::Waiting {
                        attempt: retry_attempt,
                        delay: retry_delay,
                        reason: format!("Request to {} failed: {}", provider_name, error),
                    });
                }
                wait_for_http_retry(retry_delay, cancellation).await?;
                continue;
            }
        };
        let status = response.status();

        if status.is_success() {
            if retry_attempt > 0 {
                if let Some(notify) = retry_progress {
                    notify(HttpRetryProgress::Recovered {
                        attempts: retry_attempt,
                    });
                }
            }
            return Ok(response);
        }

        let body = match response.text().await {
            Ok(body) => body,
            Err(error) => {
                retry_attempt += 1;
                if let Some(notify) = retry_progress {
                    notify(HttpRetryProgress::Waiting {
                        attempt: retry_attempt,
                        delay: retry_delay,
                        reason: format!("Failed to read {} response: {}", provider_name, error),
                    });
                }
                wait_for_http_retry(retry_delay, cancellation).await?;
                continue;
            }
        };

        if !should_retry_http_failure(status, &body) {
            return Err(format!(
                "{} returned HTTP {}: {}",
                provider_name, status, body
            ));
        }

        retry_attempt += 1;
        if let Some(notify) = retry_progress {
            notify(HttpRetryProgress::Waiting {
                attempt: retry_attempt,
                delay: retry_delay,
                reason: format!("{} returned HTTP {}: {}", provider_name, status, body),
            });
        }
        wait_for_http_retry(retry_delay, cancellation).await?;
    }
}

pub(crate) async fn send_chat_completion_request(
    client: &reqwest::Client,
    endpoint: &str,
    api_key: &str,
    provider_name: &str,
    payload: &Value,
    cancellation: Option<&AtomicBool>,
) -> Result<Value, String> {
    let response = send_http_request_with_retry(
        client,
        endpoint,
        api_key,
        provider_name,
        payload,
        cancellation,
        None,
        HTTP_RETRY_DELAY,
    )
    .await?;
    let body = response
        .text()
        .await
        .map_err(|error| format!("Failed to read {} response: {}", provider_name, error))?;

    serde_json::from_str(&body)
        .map_err(|error| format!("Failed to parse {} response: {}", provider_name, error))
}

pub(crate) async fn send_chat_completion_request_maybe_stream(
    app: &AppHandle,
    stream_id: Option<&str>,
    client: &reqwest::Client,
    endpoint: &str,
    api_key: &str,
    provider_name: &str,
    payload: &Value,
    cancellation: Option<&AtomicBool>,
    suppress_content_stream: bool,
) -> Result<Value, String> {
    if let Some(stream_id) = stream_id {
        send_chat_completion_stream_request(
            app,
            stream_id,
            client,
            endpoint,
            api_key,
            provider_name,
            payload,
            cancellation,
            suppress_content_stream,
        )
        .await
    } else {
        send_chat_completion_request(
            client,
            endpoint,
            api_key,
            provider_name,
            payload,
            cancellation,
        )
        .await
    }
}

pub(crate) async fn send_responses_request(
    client: &reqwest::Client,
    endpoint: &str,
    api_key: &str,
    provider_name: &str,
    payload: &Value,
    cancellation: Option<&AtomicBool>,
) -> Result<Value, String> {
    let response = send_http_request_with_retry(
        client,
        endpoint,
        api_key,
        provider_name,
        payload,
        cancellation,
        None,
        HTTP_RETRY_DELAY,
    )
    .await?;
    let body = response
        .text()
        .await
        .map_err(|error| format!("Failed to read {} response: {}", provider_name, error))?;

    serde_json::from_str(&body)
        .map_err(|error| format!("Failed to parse {} response: {}", provider_name, error))
}

pub(crate) async fn send_responses_request_maybe_stream(
    app: &AppHandle,
    stream_id: Option<&str>,
    client: &reqwest::Client,
    endpoint: &str,
    api_key: &str,
    provider_name: &str,
    payload: &Value,
    cancellation: Option<&AtomicBool>,
) -> Result<Value, String> {
    if let Some(stream_id) = stream_id {
        send_responses_stream_request(
            app,
            stream_id,
            client,
            endpoint,
            api_key,
            provider_name,
            payload,
            cancellation,
        )
        .await
    } else {
        send_responses_request(
            client,
            endpoint,
            api_key,
            provider_name,
            payload,
            cancellation,
        )
        .await
    }
}

pub(crate) fn emit_stream_event(
    app: &AppHandle,
    stream_id: Option<&str>,
    event_type: &str,
    trace_kind: Option<&str>,
    text: impl Into<String>,
    detail: Option<String>,
    usage: Option<ChatCompletionUsage>,
) {
    let Some(stream_id) = stream_id else {
        return;
    };

    let _ = app.emit(
        CHAT_COMPLETION_STREAM_EVENT,
        ChatCompletionStreamEvent {
            stream_id: stream_id.to_string(),
            event_type: event_type.to_string(),
            trace_kind: trace_kind.map(str::to_string),
            text: text.into(),
            detail,
            usage,
            retry_attempt: None,
            retry_delay_ms: None,
            retry_reason: None,
        },
    );
}

pub(crate) fn emit_overload_retry_event(app: &AppHandle, stream_id: &str, progress: HttpRetryProgress) {
    let (event_type, retry_attempt, retry_delay_ms, retry_reason) = match progress {
        HttpRetryProgress::Waiting {
            attempt,
            delay,
            reason,
        } => (
            "retryWaiting",
            attempt,
            Some(delay.as_millis().min(u64::MAX as u128) as u64),
            Some(reason),
        ),
        HttpRetryProgress::Recovered { attempts } => ("retryRecovered", attempts, None, None),
    };

    let _ = app.emit(
        CHAT_COMPLETION_STREAM_EVENT,
        ChatCompletionStreamEvent {
            stream_id: stream_id.to_string(),
            event_type: event_type.to_string(),
            trace_kind: None,
            text: String::new(),
            detail: None,
            usage: None,
            retry_attempt: Some(retry_attempt),
            retry_delay_ms,
            retry_reason,
        },
    );
}

/// If tool_call is a successful dispatch_tasks invocation, return the task entries.
pub(crate) fn extract_dispatched_tasks(tool_call: &Value, tool_result: &Value) -> Option<Vec<TaskDispatchedEntry>> {
    let function = tool_call.get("function")?;
    let name = function.get("name")?.as_str()?;
    if name != "dispatch_tasks" {
        return None;
    }

    if !ValidationOps::result_succeeded(tool_result) {
        return None;
    }

    let arguments: Value = function
        .get("arguments")
        .and_then(|a| serde_json::from_str(a.as_str()?).ok())?;
    let tasks = arguments.get("tasks")?.as_array()?;

    let entries: Vec<TaskDispatchedEntry> = tasks
        .iter()
        .filter_map(|task| {
            let member = task
                .get("member")?
                .as_str()?
                .trim()
                .trim_start_matches('@')
                .to_string();
            let instruction = task.get("instruction")?.as_str()?.trim().to_string();
            if member.is_empty() || instruction.is_empty() {
                return None;
            }
            Some(TaskDispatchedEntry { member, instruction })
        })
        .collect();

    if entries.is_empty() { None } else { Some(entries) }
}

pub(crate) fn should_finish_after_dispatch_tasks(
    dispatched_tasks: Option<&[TaskDispatchedEntry]>,
    validation: &ValidationState,
    edit_recovery_required: bool,
) -> bool {
    dispatched_tasks.is_some()
        && !edit_recovery_required
        && !validation.requires_tool(edit_recovery_required)
        && !validation.requires_repair()
}

pub(crate) fn dispatched_tasks_completion_content(entries: &[TaskDispatchedEntry]) -> String {
    entries
        .iter()
        .map(|entry| format!("- @{}: {}", entry.member, entry.instruction))
        .collect::<Vec<_>>()
        .join("\n")
}

pub(crate) fn emit_trace_step(app: &AppHandle, stream_id: Option<&str>, step: &ChatTraceStep) {
    emit_stream_event(
        app,
        stream_id,
        "traceStep",
        Some(&step.kind),
        step.text.clone(),
        step.detail.clone(),
        None,
    );
}

/// Surfaces an internal instruction that the loop injects into the model
/// messages (system prompt, phase/validation/recovery/nudge instructions) as a
/// trace step, so the user can watch exactly what the agent was told.
pub(crate) fn emit_instruction_step(
    app: &AppHandle,
    stream_id: Option<&str>,
    trace_steps: &mut Vec<ChatTraceStep>,
    label: &str,
    instruction: &str,
) {
    let step = ChatTraceStep {
        kind: "instruction".to_string(),
        text: format!("Internal instruction injected: {label}"),
        detail: Some(instruction.to_string()),
    };
    emit_trace_step(app, stream_id, &step);
    TraceCtx::append_steps(trace_steps, vec![step]);
}

pub(crate) fn emit_tool_chunk(app: &AppHandle, stream_id: Option<&str>, step: &ChatTraceStep) {
    emit_stream_event(
        app,
        stream_id,
        "toolChunk",
        Some(&step.kind),
        step.text.clone(),
        step.detail.clone(),
        None,
    );
}

pub(crate) fn emit_trace_chunk(app: &AppHandle, stream_id: &str, trace_kind: &str, text: &str) {
    emit_stream_event(
        app,
        Some(stream_id),
        "traceChunk",
        Some(trace_kind),
        text,
        None,
        None,
    );
}

pub(crate) fn emit_content_chunk(app: &AppHandle, stream_id: &str, text: &str) {
    emit_stream_event(app, Some(stream_id), "contentChunk", None, text, None, None);
}

pub(crate) fn emit_usage_event(app: &AppHandle, stream_id: &str, usage: ChatCompletionUsage) {
    emit_stream_event(app, Some(stream_id), "usage", None, "", None, Some(usage));
}

pub(crate) fn sse_event_separator(buffer: &str) -> Option<(usize, usize)> {
    match (buffer.find("\n\n"), buffer.find("\r\n\r\n")) {
        (Some(lf), Some(crlf)) if crlf < lf => Some((crlf, 4)),
        (Some(lf), _) => Some((lf, 2)),
        (_, Some(crlf)) => Some((crlf, 4)),
        _ => None,
    }
}

pub(crate) fn sse_data_lines(event_block: &str) -> Vec<String> {
    event_block
        .lines()
        .filter_map(|line| {
            let line = line.trim_end_matches('\r');
            line.strip_prefix("data:")
                .map(|data| data.trim_start().to_string())
        })
        .collect()
}

pub(crate) fn ensure_tool_call_slot(tool_calls: &mut Vec<ToolCallAccumulator>, index: usize) {
    while tool_calls.len() <= index {
        tool_calls.push(ToolCallAccumulator::default());
    }
}

pub(crate) fn append_delta_tool_calls(delta_tool_calls: &[Value], tool_calls: &mut Vec<ToolCallAccumulator>) {
    for delta_call in delta_tool_calls {
        let index = delta_call
            .get("index")
            .and_then(Value::as_u64)
            .unwrap_or(tool_calls.len() as u64) as usize;
        ensure_tool_call_slot(tool_calls, index);
        let accumulator = &mut tool_calls[index];

        if let Some(id) = delta_call.get("id").and_then(Value::as_str) {
            accumulator.id.push_str(id);
        }

        if let Some(call_type) = delta_call.get("type").and_then(Value::as_str) {
            accumulator.call_type.push_str(call_type);
        }

        if let Some(function) = delta_call.get("function") {
            if let Some(name) = function.get("name").and_then(Value::as_str) {
                accumulator.function_name.push_str(name);
            }

            if let Some(arguments) = function.get("arguments").and_then(Value::as_str) {
                accumulator.function_arguments.push_str(arguments);
            }
        }
    }
}

pub(crate) fn tool_call_accumulators_to_values(tool_calls: Vec<ToolCallAccumulator>) -> Vec<Value> {
    tool_calls
        .into_iter()
        .enumerate()
        .filter(|(_, call)| {
            !call.function_name.trim().is_empty() || !call.function_arguments.trim().is_empty()
        })
        .map(|(index, call)| {
            json!({
                "id": if call.id.is_empty() {
                    format!("streamed-tool-call-{}", index)
                } else {
                    call.id
                },
                "type": if call.call_type.is_empty() {
                    "function".to_string()
                } else {
                    call.call_type
                },
                "function": {
                    "name": call.function_name,
                    "arguments": call.function_arguments,
                },
            })
        })
        .collect()
}

pub(crate) fn apply_stream_delta(
    app: &AppHandle,
    stream_id: &str,
    parsed: &Value,
    content: &mut String,
    reasoning: &mut String,
    tool_calls: &mut Vec<ToolCallAccumulator>,
    finish_reason: &mut Option<String>,
    usage: &mut Option<ChatCompletionUsage>,
    suppress_content_stream: bool,
) {
    if let Some(next_usage) = TraceCtx::usage_from(parsed) {
        emit_usage_event(app, stream_id, next_usage.clone());
        *usage = Some(next_usage);
    }

    let Some(choices) = parsed.get("choices").and_then(Value::as_array) else {
        return;
    };

    for choice in choices {
        if let Some(reason) = choice.get("finish_reason").and_then(Value::as_str) {
            *finish_reason = Some(reason.to_string());
        }

        let Some(delta) = choice.get("delta") else {
            continue;
        };

        let reasoning_chunk = ["reasoning_content", "reasoning"]
            .into_iter()
            .filter_map(|key| delta.get(key).and_then(Value::as_str))
            .find(|chunk| !chunk.is_empty());

        if let Some(chunk) = reasoning_chunk {
            reasoning.push_str(chunk);
            emit_trace_chunk(app, stream_id, "reasoning", chunk);
        }

        if let Some(chunk) = delta.get("content").and_then(Value::as_str) {
            if !chunk.is_empty() {
                content.push_str(chunk);
                if !suppress_content_stream {
                    emit_content_chunk(app, stream_id, chunk);
                }
            }
        }

        if let Some(delta_tool_calls) = delta.get("tool_calls").and_then(Value::as_array) {
            append_delta_tool_calls(delta_tool_calls, tool_calls);
        }
    }
}

pub(crate) async fn send_chat_completion_stream_request(
    app: &AppHandle,
    stream_id: &str,
    client: &reqwest::Client,
    endpoint: &str,
    api_key: &str,
    provider_name: &str,
    payload: &Value,
    cancellation: Option<&AtomicBool>,
    suppress_content_stream: bool,
) -> Result<Value, String> {
    let mut payload = payload.clone();
    payload["stream"] = json!(true);
    payload["stream_options"] = json!({ "include_usage": true });
    let report_retry = |progress| emit_overload_retry_event(app, stream_id, progress);

    let response = send_http_request_with_retry(
        client,
        endpoint,
        api_key,
        provider_name,
        &payload,
        cancellation,
        Some(&report_retry),
        HTTP_RETRY_DELAY,
    )
    .await?;

    let mut content = String::new();
    let mut reasoning = String::new();
    let mut tool_calls: Vec<ToolCallAccumulator> = Vec::new();
    let mut finish_reason: Option<String> = None;
    let mut usage: Option<ChatCompletionUsage> = None;
    let mut buffer = String::new();
    let mut stream = response.bytes_stream();

    while let Some(chunk) = stream.next().await {
        let bytes =
            chunk.map_err(|error| format!("Failed to read {} stream: {}", provider_name, error))?;
        buffer.push_str(&String::from_utf8_lossy(&bytes));

        while let Some((separator_index, separator_len)) = sse_event_separator(&buffer) {
            let event_block = buffer[..separator_index].to_string();
            buffer = buffer[(separator_index + separator_len)..].to_string();

            for data in sse_data_lines(&event_block) {
                if data == "[DONE]" {
                    break;
                }

                let parsed: Value = serde_json::from_str(&data).map_err(|error| {
                    format!("Failed to parse {} stream event: {}", provider_name, error)
                })?;
                apply_stream_delta(
                    app,
                    stream_id,
                    &parsed,
                    &mut content,
                    &mut reasoning,
                    &mut tool_calls,
                    &mut finish_reason,
                    &mut usage,
                    suppress_content_stream,
                );
            }
        }
    }

    if !buffer.trim().is_empty() {
        for data in sse_data_lines(&buffer) {
            if data == "[DONE]" {
                continue;
            }

            let parsed: Value = serde_json::from_str(&data).map_err(|error| {
                format!(
                    "Failed to parse {} final stream event: {}",
                    provider_name, error
                )
            })?;
            apply_stream_delta(
                app,
                stream_id,
                &parsed,
                &mut content,
                &mut reasoning,
                &mut tool_calls,
                &mut finish_reason,
                &mut usage,
                suppress_content_stream,
            );
        }
    }

    let mut message = json!({
        "role": "assistant",
        "content": content,
    });

    if !reasoning.trim().is_empty() {
        message["reasoning_content"] = json!(reasoning);
    }

    let tool_calls = tool_call_accumulators_to_values(tool_calls);

    if !tool_calls.is_empty() {
        message["tool_calls"] = Value::Array(tool_calls);
    }

    Ok(json!({
        "choices": [
            {
                "message": message,
                "finish_reason": finish_reason,
            }
        ],
        "usage": usage
    }))
}

pub(crate) fn response_stream_error_text(parsed: &Value) -> Option<String> {
    let error = parsed
        .get("response")
        .and_then(|response| response.get("error"))
        .or_else(|| parsed.get("error"))?;

    error
        .get("message")
        .and_then(Value::as_str)
        .or_else(|| error.as_str())
        .map(str::to_string)
}

pub(crate) fn collect_response_reasoning_summary(item: &Value) -> String {
    item.get("summary")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|summary| {
            summary
                .get("text")
                .or_else(|| summary.get("content"))
                .and_then(Value::as_str)
        })
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

pub(crate) fn response_stream_text(parsed: &Value) -> Option<&str> {
    parsed
        .get("delta")
        .and_then(Value::as_str)
        .or_else(|| parsed.get("text").and_then(Value::as_str))
        .or_else(|| {
            parsed.get("part").and_then(|part| {
                part.get("text")
                    .or_else(|| part.get("content"))
                    .and_then(Value::as_str)
            })
        })
}

pub(crate) fn append_response_reasoning(
    reasoning: &mut String,
    next_text: &str,
    separate_complete_fragment: bool,
) -> Option<String> {
    if next_text.is_empty() || reasoning.ends_with(next_text) {
        return None;
    }

    if !reasoning.is_empty() && next_text.starts_with(reasoning.as_str()) {
        let delta = next_text[reasoning.len()..].to_string();

        if delta.is_empty() {
            return None;
        }

        reasoning.push_str(&delta);
        return Some(delta);
    }

    let mut emitted = String::new();

    if separate_complete_fragment
        && !reasoning.is_empty()
        && !reasoning.ends_with('\n')
        && !next_text.starts_with('\n')
    {
        reasoning.push('\n');
        emitted.push('\n');
    }

    reasoning.push_str(next_text);
    emitted.push_str(next_text);
    Some(emitted)
}

pub(crate) fn apply_responses_stream_event(
    app: &AppHandle,
    stream_id: &str,
    parsed: &Value,
    content: &mut String,
    reasoning: &mut String,
    function_calls: &mut Vec<Value>,
    response_id: &mut Option<String>,
    usage: &mut Option<ChatCompletionUsage>,
    completed_response: &mut Option<Value>,
) -> Option<String> {
    if let Some(id) = parsed
        .get("response")
        .and_then(|response| response.get("id"))
        .and_then(Value::as_str)
        .or_else(|| parsed.get("id").and_then(Value::as_str))
    {
        *response_id = Some(id.to_string());
    }

    if let Some(response) = parsed.get("response") {
        if let Some(next_usage) = TraceCtx::usage_from(response) {
            emit_usage_event(app, stream_id, next_usage.clone());
            *usage = Some(next_usage);
        }
    }

    let event_type = parsed.get("type").and_then(Value::as_str).unwrap_or("");

    match event_type {
        "response.output_text.delta" => {
            if let Some(delta) = parsed.get("delta").and_then(Value::as_str) {
                if !delta.is_empty() {
                    content.push_str(delta);
                    emit_content_chunk(app, stream_id, delta);
                }
            }
        }
        "response.reasoning_summary_text.delta" | "response.reasoning_text.delta" => {
            if let Some(delta) = response_stream_text(parsed) {
                if let Some(emitted) = append_response_reasoning(reasoning, delta, false) {
                    emit_trace_chunk(app, stream_id, "reasoning", &emitted);
                }
            }
        }
        "response.reasoning_summary_part.added"
        | "response.reasoning_summary_part.done"
        | "response.reasoning_summary_text.done"
        | "response.reasoning_text.done" => {
            if let Some(text) = response_stream_text(parsed) {
                if let Some(emitted) = append_response_reasoning(reasoning, text, true) {
                    emit_trace_chunk(app, stream_id, "reasoning", &emitted);
                }
            }
        }
        "response.output_item.done" => {
            if let Some(item) = parsed.get("item") {
                if item.get("type").and_then(Value::as_str) == Some("function_call") {
                    function_calls.push(item.clone());
                } else if item.get("type").and_then(Value::as_str) == Some("reasoning") {
                    let summary = collect_response_reasoning_summary(item);
                    if !summary.is_empty() {
                        if let Some(emitted) = append_response_reasoning(reasoning, &summary, true)
                        {
                            emit_trace_chunk(app, stream_id, "reasoning", &emitted);
                        }
                    }
                }
            }
        }
        "response.completed" => {
            if let Some(response) = parsed.get("response") {
                if let Some(next_usage) = TraceCtx::usage_from(response) {
                    emit_usage_event(app, stream_id, next_usage.clone());
                    *usage = Some(next_usage);
                }
                *completed_response = Some(response.clone());
            }
        }
        "response.failed" | "response.incomplete" => {
            return response_stream_error_text(parsed);
        }
        _ => {}
    }

    None
}

pub(crate) async fn send_responses_stream_request(
    app: &AppHandle,
    stream_id: &str,
    client: &reqwest::Client,
    endpoint: &str,
    api_key: &str,
    provider_name: &str,
    payload: &Value,
    cancellation: Option<&AtomicBool>,
) -> Result<Value, String> {
    let mut payload = payload.clone();
    payload["stream"] = json!(true);
    let report_retry = |progress| emit_overload_retry_event(app, stream_id, progress);

    let response = send_http_request_with_retry(
        client,
        endpoint,
        api_key,
        provider_name,
        &payload,
        cancellation,
        Some(&report_retry),
        HTTP_RETRY_DELAY,
    )
    .await?;

    let mut content = String::new();
    let mut reasoning = String::new();
    let mut function_calls = Vec::new();
    let mut response_id: Option<String> = None;
    let mut usage: Option<ChatCompletionUsage> = None;
    let mut completed_response: Option<Value> = None;
    let mut buffer = String::new();
    let mut stream = response.bytes_stream();

    while let Some(chunk) = stream.next().await {
        let bytes =
            chunk.map_err(|error| format!("Failed to read {} stream: {}", provider_name, error))?;
        buffer.push_str(&String::from_utf8_lossy(&bytes));

        while let Some((separator_index, separator_len)) = sse_event_separator(&buffer) {
            let event_block = buffer[..separator_index].to_string();
            buffer = buffer[(separator_index + separator_len)..].to_string();

            for data in sse_data_lines(&event_block) {
                if data == "[DONE]" {
                    break;
                }

                let parsed: Value = serde_json::from_str(&data).map_err(|error| {
                    format!("Failed to parse {} stream event: {}", provider_name, error)
                })?;

                if let Some(error) = apply_responses_stream_event(
                    app,
                    stream_id,
                    &parsed,
                    &mut content,
                    &mut reasoning,
                    &mut function_calls,
                    &mut response_id,
                    &mut usage,
                    &mut completed_response,
                ) {
                    return Err(format!("{} response failed: {}", provider_name, error));
                }
            }
        }
    }

    if !buffer.trim().is_empty() {
        for data in sse_data_lines(&buffer) {
            if data == "[DONE]" {
                continue;
            }

            let parsed: Value = serde_json::from_str(&data).map_err(|error| {
                format!(
                    "Failed to parse {} final stream event: {}",
                    provider_name, error
                )
            })?;

            if let Some(error) = apply_responses_stream_event(
                app,
                stream_id,
                &parsed,
                &mut content,
                &mut reasoning,
                &mut function_calls,
                &mut response_id,
                &mut usage,
                &mut completed_response,
            ) {
                return Err(format!("{} response failed: {}", provider_name, error));
            }
        }
    }

    if let Some(response) = completed_response {
        return Ok(response);
    }

    let mut output = Vec::new();
    if !reasoning.trim().is_empty() {
        output.push(json!({
            "type": "reasoning",
            "summary": [{ "type": "summary_text", "text": reasoning }],
        }));
    }
    if !content.trim().is_empty() {
        output.push(json!({
            "type": "message",
            "content": [{ "type": "output_text", "text": content }],
        }));
    }
    output.extend(function_calls);

    Ok(json!({
        "id": response_id,
        "output": output,
        "output_text": content,
        "usage": usage,
    }))
}

