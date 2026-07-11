use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ToolCallExecutionPlan {
    pub(crate) executable_count: usize,
    pub(crate) truncated: bool,
}

pub(crate) struct ToolCallUtils;

impl ToolCallUtils {
    pub(crate) fn can_open_next_autonomous_round(current_round: usize, max_rounds: usize) -> bool {
        current_round < max_rounds
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

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
        assert!(ToolCallUtils::can_open_next_autonomous_round(1, 4));
        assert!(ToolCallUtils::can_open_next_autonomous_round(3, 4));
        assert!(!ToolCallUtils::can_open_next_autonomous_round(4, 4));
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
