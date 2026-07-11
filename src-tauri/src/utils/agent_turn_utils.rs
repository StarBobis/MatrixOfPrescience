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
}

impl AgentTurnState {
    pub(crate) const DEEPSEEK_REFLECTION_MAX_TOKENS: usize = 768;
    pub(crate) const DEEPSEEK_REFLECTION_INSTRUCTION: &'static str = "Reflection step after a tool result. Do not call tools on this turn. Decide whether the user's task is complete. Reply with exactly one control line followed by concise content: `FINAL` then the user-facing final answer when the task is complete and validated, or `CONTINUE` then the single best next tool action when more work is needed. Do not write patches or tool arguments in this reflection.";

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

        if self.deepseek_tool_workflow {
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
    }

    pub(crate) fn complete_reflection(&mut self) {
        self.reflection_pending = false;
    }

    pub(crate) fn thinking_enabled(&self, phase: AgentTurnPhase) -> bool {
        if !self.deepseek_reasoning_requested {
            return false;
        }

        if !self.deepseek_tool_workflow {
            return true;
        }

        matches!(
            phase,
            AgentTurnPhase::ToolReflection
                | AgentTurnPhase::BudgetCheckpoint
                | AgentTurnPhase::FinalAnswer
        )
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
        if validation_tool_required || repair_required || orchestration_required {
            return Some(json!("required"));
        }

        if self.thinking_enabled(phase) {
            return None;
        }

        if self.is_deepseek && self.deepseek_tool_workflow && phase == AgentTurnPhase::ToolAction {
            return Some(json!("required"));
        }

        Some(json!("auto"))
    }

    pub(crate) fn parse_reflection(content: &str) -> AgentReflectionDecision {
        let trimmed = content.trim();
        let (control, following_lines) = trimmed.split_once('\n').unwrap_or((trimmed, ""));
        let control = control.trim().trim_matches('*').trim();

        let ascii_final_prefix_len = "FINAL:".len();
        let inline_answer = if control
            .get(..ascii_final_prefix_len)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("FINAL:"))
        {
            Some(
                control
                    .get(ascii_final_prefix_len..)
                    .unwrap_or("")
                    .trim()
                    .trim_start_matches('*')
                    .trim(),
            )
        } else if control.starts_with("FINAL：") {
            Some(
                control
                    .get("FINAL：".len()..)
                    .unwrap_or("")
                    .trim()
                    .trim_start_matches('*')
                    .trim(),
            )
        } else {
            None
        };

        if !control.eq_ignore_ascii_case("FINAL") && inline_answer.is_none() {
            return AgentReflectionDecision::Continue;
        }

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deepseek_cycles_from_action_to_reflection_and_back() {
        let mut state = AgentTurnState::new(true, true, true);

        assert_eq!(
            state.phase(false, false, false, false),
            AgentTurnPhase::ToolAction
        );
        assert_eq!(
            state.tool_choice(AgentTurnPhase::ToolAction, false, false, false),
            Some(json!("required"))
        );
        assert!(!state.thinking_enabled(AgentTurnPhase::ToolAction));

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
            Some(json!("required"))
        );
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
        assert!(!state.thinking_enabled(AgentTurnPhase::ToolAction));
        assert_eq!(
            state.tool_choice(AgentTurnPhase::ToolAction, false, true, false),
            Some(json!("required"))
        );
    }
}
