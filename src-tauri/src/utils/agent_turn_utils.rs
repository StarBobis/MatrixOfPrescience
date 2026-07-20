use serde_json::{json, Value};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AgentTurnPhase {
    Conversation,
    ToolAction,
    ToolReflection,
    BudgetCheckpoint,
    FinalAnswer,
}

impl AgentTurnPhase {
    pub(crate) fn blocks_tools(self) -> bool {
        matches!(
            self,
            Self::ToolReflection | Self::BudgetCheckpoint | Self::FinalAnswer
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AgentReflectionDecision {
    Continue,
    Finish(String),
    RequestFinalAnswer,
}

pub(crate) struct AgentTurnState {
    is_deepseek: bool,
    deepseek_reasoning_requested: bool,
    deepseek_tool_workflow: bool,
    reflection_pending: bool,
    tools_executed: bool,
    unresolved_reflections: usize,
}

impl AgentTurnState {
    pub(crate) const DEEPSEEK_REFLECTION_MAX_TOKENS: usize = 768;
    pub(crate) const MAX_UNRESOLVED_REFLECTIONS: usize = 2;
    const REFLECTION_CONTROL_SCAN_LINES: usize = 8;
    pub(crate) const DEEPSEEK_REFLECTION_INSTRUCTION: &'static str = "Reflection step after a tool result. Do not call tools on this turn. Decide whether the user's task is complete. Start the reply with exactly one control line, then concise content: `FINAL` then the user-facing final answer when the task is complete and validated, or `CONTINUE` then the single best next tool action when more work is needed. Do not write patches or tool arguments in this reflection.";

    pub(crate) fn new(
        is_deepseek: bool,
        deepseek_reasoning_requested: bool,
        deepseek_tool_workflow: bool,
    ) -> Self {
        Self {
            is_deepseek,
            deepseek_reasoning_requested,
            deepseek_tool_workflow,
            reflection_pending: false,
            tools_executed: false,
            unresolved_reflections: 0,
        }
    }

    pub(crate) fn phase(
        &self,
        final_answer_requested: bool,
        budget_checkpoint_pending: bool,
        validation_tool_required: bool,
        repair_required: bool,
    ) -> AgentTurnPhase {
        if validation_tool_required || repair_required {
            return AgentTurnPhase::ToolAction;
        }

        if final_answer_requested {
            return AgentTurnPhase::FinalAnswer;
        }

        if self.reflection_pending {
            return AgentTurnPhase::ToolReflection;
        }

        if budget_checkpoint_pending {
            return AgentTurnPhase::BudgetCheckpoint;
        }

        // Only push the model into the action phase after it actually called a
        // tool. The first turn stays a free conversation turn so the model can
        // answer directly (or stop) whenever the task needs no tool work.
        if self.deepseek_tool_workflow && self.tools_executed {
            AgentTurnPhase::ToolAction
        } else {
            AgentTurnPhase::Conversation
        }
    }

    pub(crate) fn max_turns(&self, default_max_turns: usize) -> usize {
        if self.deepseek_tool_workflow {
            default_max_turns.saturating_mul(2)
        } else {
            default_max_turns
        }
    }

    pub(crate) fn mark_tools_executed(&mut self) {
        if self.deepseek_tool_workflow {
            self.reflection_pending = true;
        }
        self.tools_executed = true;
        self.unresolved_reflections = 0;
    }

    pub(crate) fn complete_reflection(&mut self) {
        self.reflection_pending = false;
    }

    /// Counts a reflection that ended without a FINAL decision. Returns true
    /// once the unresolved streak hits the limit, so the caller can ask for a
    /// direct final answer instead of letting the action/reflection loop spin.
    pub(crate) fn record_unresolved_reflection(&mut self) -> bool {
        self.unresolved_reflections += 1;
        self.unresolved_reflections >= Self::MAX_UNRESOLVED_REFLECTIONS
    }

    pub(crate) fn thinking_enabled(&self, _phase: AgentTurnPhase) -> bool {
        self.deepseek_reasoning_requested
    }

    pub(crate) fn reasoning_effort<'a>(
        &self,
        phase: AgentTurnPhase,
        request_reasoning_effort: Option<&'a str>,
        checkpoint_reasoning_effort: &'a str,
    ) -> Option<&'a str> {
        if self.is_deepseek && !self.thinking_enabled(phase) {
            return Some("off");
        }

        if phase == AgentTurnPhase::BudgetCheckpoint {
            return Some(checkpoint_reasoning_effort);
        }

        request_reasoning_effort
    }

    pub(crate) fn tool_choice(
        &self,
        phase: AgentTurnPhase,
        validation_tool_required: bool,
        repair_required: bool,
        orchestration_required: bool,
    ) -> Option<Value> {
        // DeepSeek thinking mode rejects tool_choice: "required" under any condition.
        // Return None so no tool_choice is sent; the model defaults to whatever it
        // prefers (including DSML tool calls in content). The phase instruction
        // (DEEPSEEK_TOOL_ACTION_INSTRUCTION / VALIDATION_REQUIRED_INSTRUCTION /
        // EDIT_FAILURE_RECOVERY_INSTRUCTION) guides the model to call the appropriate tool.
        if self.is_deepseek && self.deepseek_tool_workflow && self.thinking_enabled(phase) {
            return None;
        }

        if validation_tool_required || repair_required || orchestration_required {
            return Some(json!("required"));
        }

        Some(json!("auto"))
    }

    /// Extracts the reflection decision. The control line may carry markdown
    /// decoration (`**FINAL:**`, `- FINAL:`, `1. FINAL:`) and may appear after
    /// a few lead-in lines; anything without a FINAL control line keeps the
    /// loop going with Continue.
    pub(crate) fn parse_reflection(content: &str) -> AgentReflectionDecision {
        let trimmed = content.trim();
        let lines: Vec<&str> = trimmed.lines().collect();
        let mut control_index = None;

        for (index, line) in lines
            .iter()
            .take(Self::REFLECTION_CONTROL_SCAN_LINES)
            .enumerate()
        {
            if final_control_line(strip_control_line_decoration(line)).is_some() {
                control_index = Some(index);
                break;
            }
        }

        let Some(control_index) = control_index else {
            return AgentReflectionDecision::Continue;
        };

        let inline_answer =
            final_control_line(strip_control_line_decoration(lines[control_index])).flatten();
        let following_lines = lines[control_index + 1..].join("\n");

        let answer = match inline_answer {
            Some(inline_answer) if !inline_answer.is_empty() => inline_answer,
            _ => following_lines.trim(),
        };
        if answer.is_empty() {
            AgentReflectionDecision::RequestFinalAnswer
        } else {
            AgentReflectionDecision::Finish(answer.to_string())
        }
    }
}

/// Removes markdown list/emphasis decoration (`**`, `- `, `1. `, `2) `) from
/// the start of a candidate control line.
fn strip_control_line_decoration(line: &str) -> &str {
    let mut text = line.trim();

    loop {
        let stripped = text
            .trim_start_matches(['*', '-', '+', '#', '>'])
            .trim_start();
        if stripped.len() == text.len() {
            break;
        }
        text = stripped;
    }

    let digit_len = text.len() - text.trim_start_matches(|c: char| c.is_ascii_digit()).len();
    if digit_len > 0 {
        if let Some(rest) = text[digit_len..].trim_start().strip_prefix(['.', ')']) {
            text = rest.trim_start();
        }
    }

    text.trim_matches('*').trim()
}

/// Returns `Some(None)` for a bare `FINAL` line and `Some(Some(answer))` for
/// `FINAL: answer` / `FINAL：answer`; `None` when the line is not a FINAL control.
fn final_control_line(candidate: &str) -> Option<Option<&str>> {
    const ASCII_FINAL_PREFIX: &str = "FINAL:";

    if candidate.eq_ignore_ascii_case("FINAL") {
        return Some(None);
    }

    if candidate
        .get(..ASCII_FINAL_PREFIX.len())
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(ASCII_FINAL_PREFIX))
    {
        return Some(Some(
            candidate
                .get(ASCII_FINAL_PREFIX.len()..)
                .unwrap_or("")
                .trim()
                .trim_start_matches('*')
                .trim(),
        ));
    }

    if let Some(rest) = candidate.strip_prefix("FINAL：") {
        return Some(Some(rest.trim().trim_start_matches('*').trim()));
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deepseek_first_turn_is_a_free_conversation_turn() {
        let state = AgentTurnState::new(true, true, true);

        assert_eq!(
            state.phase(false, false, false, false),
            AgentTurnPhase::Conversation
        );
        assert!(state.thinking_enabled(AgentTurnPhase::Conversation));
    }

    #[test]
    fn deepseek_cycles_from_action_to_reflection_and_back() {
        let mut state = AgentTurnState::new(true, true, true);

        assert_eq!(
            state.phase(false, false, false, false),
            AgentTurnPhase::Conversation
        );

        state.mark_tools_executed();
        assert_eq!(
            state.phase(false, false, false, false),
            AgentTurnPhase::ToolReflection
        );
        assert!(state.thinking_enabled(AgentTurnPhase::ToolReflection));

        state.complete_reflection();
        assert_eq!(
            state.phase(false, false, false, false),
            AgentTurnPhase::ToolAction
        );
        assert_eq!(
            state.tool_choice(AgentTurnPhase::ToolAction, false, false, false),
            None
        );
        assert!(state.thinking_enabled(AgentTurnPhase::ToolAction));
    }

    #[test]
    fn unresolved_reflections_eventually_request_a_direct_answer() {
        let mut state = AgentTurnState::new(true, true, true);

        assert!(!state.record_unresolved_reflection());
        assert!(state.record_unresolved_reflection());

        state.mark_tools_executed();
        assert!(!state.record_unresolved_reflection());
    }

    #[test]
    fn reflection_can_finish_or_continue_without_user_input() {
        assert_eq!(
            AgentTurnState::parse_reflection("CONTINUE\nRead the next source section."),
            AgentReflectionDecision::Continue
        );
        assert_eq!(
            AgentTurnState::parse_reflection("FINAL\nThe requested change is complete."),
            AgentReflectionDecision::Finish("The requested change is complete.".to_string())
        );
        assert_eq!(
            AgentTurnState::parse_reflection("**FINAL:** The requested change is complete."),
            AgentReflectionDecision::Finish("The requested change is complete.".to_string())
        );
        assert_eq!(
            AgentTurnState::parse_reflection("FINAL"),
            AgentReflectionDecision::RequestFinalAnswer
        );
    }

    #[test]
    fn reflection_control_line_tolerates_markdown_and_lead_in_lines() {
        assert_eq!(
            AgentTurnState::parse_reflection("- FINAL: done"),
            AgentReflectionDecision::Finish("done".to_string())
        );
        assert_eq!(
            AgentTurnState::parse_reflection("1. FINAL: done"),
            AgentReflectionDecision::Finish("done".to_string())
        );
        assert_eq!(
            AgentTurnState::parse_reflection("思考结论如下：\nFINAL：准备好了"),
            AgentReflectionDecision::Finish("准备好了".to_string())
        );
        assert_eq!(
            AgentTurnState::parse_reflection("FINAL:\n多行\n答案"),
            AgentReflectionDecision::Finish("多行\n答案".to_string())
        );
        assert_eq!(
            AgentTurnState::parse_reflection(
                "I should output FINAL with a very concise answer. The user wants a tool."
            ),
            AgentReflectionDecision::Continue
        );
    }

    #[test]
    fn deepseek_tool_reflections_do_not_reduce_tool_capacity() {
        let deepseek_state = AgentTurnState::new(true, true, true);
        let regular_state = AgentTurnState::new(false, false, false);

        assert_eq!(deepseek_state.max_turns(32), 64);
        assert_eq!(regular_state.max_turns(32), 32);
    }

    #[test]
    fn repair_action_temporarily_overrides_pending_reflection() {
        let mut state = AgentTurnState::new(true, true, true);
        state.mark_tools_executed();

        assert_eq!(
            state.phase(false, false, false, true),
            AgentTurnPhase::ToolAction
        );
        assert!(state.thinking_enabled(AgentTurnPhase::ToolAction));
        assert_eq!(
            state.tool_choice(AgentTurnPhase::ToolAction, false, true, false),
            None
        );
    }
}
