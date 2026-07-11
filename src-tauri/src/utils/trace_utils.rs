use serde::Serialize;
use serde_json::Value;

use crate::utils::string_utils::StrUtils;

pub(crate) const MAX_CHAT_TRACE_STEPS: usize = 160;

#[derive(Debug, Serialize, Clone)]
pub(crate) struct ChatTraceStep {
    pub(crate) kind: String,
    pub(crate) text: String,
    pub(crate) detail: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ChatCompletionUsage {
    pub(crate) prompt_tokens: Option<u64>,
    pub(crate) completion_tokens: Option<u64>,
    pub(crate) total_tokens: Option<u64>,
    pub(crate) prompt_cache_hit_tokens: Option<u64>,
    pub(crate) prompt_cache_miss_tokens: Option<u64>,
}

pub(crate) struct TraceCtx;

impl TraceCtx {
    pub(crate) fn trace_step(kind: &str, text: String) -> ChatTraceStep {
        ChatTraceStep {
            kind: kind.to_string(),
            text,
            detail: None,
        }
    }

    pub(crate) fn message_reasoning(message: &Value) -> String {
        for key in ["reasoning_content", "reasoning"] {
            if let Some(value) = message.get(key) {
                let text = StrUtils::extract_value_text(value);

                if !text.is_empty() {
                    return text;
                }
            }
        }

        String::new()
    }

    pub(crate) fn split_trace(text: &str) -> Vec<String> {
        let mut steps = Vec::new();

        for raw_line in text.lines() {
            let line = raw_line.trim();

            if line.is_empty() {
                continue;
            }

            let chars: Vec<char> = line.chars().collect();

            if chars.len() <= crate::utils::string_utils::TRACE_STEP_TEXT_LIMIT {
                steps.push(line.to_string());
                continue;
            }

            for chunk in chars.chunks(crate::utils::string_utils::TRACE_STEP_TEXT_LIMIT) {
                steps.push(chunk.iter().collect());
            }
        }

        steps
    }

    pub(crate) fn append_steps(
        trace_steps: &mut Vec<ChatTraceStep>,
        next_steps: Vec<ChatTraceStep>,
    ) {
        for step in next_steps {
            if trace_steps.len() >= MAX_CHAT_TRACE_STEPS {
                return;
            }

            if !step.text.trim().is_empty() {
                trace_steps.push(step);
            }
        }
    }

    pub(crate) fn append_reasoning(trace_steps: &mut Vec<ChatTraceStep>, message: &Value) {
        let reasoning = Self::message_reasoning(message);
        let steps = Self::split_trace(&reasoning)
            .into_iter()
            .map(|line| Self::trace_step("reasoning", line))
            .collect();

        Self::append_steps(trace_steps, steps);
    }

    pub(crate) fn usage_from(parsed: &Value) -> Option<ChatCompletionUsage> {
        let usage = parsed.get("usage")?;
        let input_tokens = usage.get("input_tokens").and_then(Value::as_u64);
        let output_tokens = usage.get("output_tokens").and_then(Value::as_u64);
        let cached_tokens = usage
            .get("input_tokens_details")
            .and_then(|details| details.get("cached_tokens"))
            .and_then(Value::as_u64);

        Some(ChatCompletionUsage {
            prompt_tokens: usage
                .get("prompt_tokens")
                .and_then(Value::as_u64)
                .or(input_tokens),
            completion_tokens: usage
                .get("completion_tokens")
                .and_then(Value::as_u64)
                .or(output_tokens),
            total_tokens: usage
                .get("total_tokens")
                .and_then(Value::as_u64)
                .or_else(|| input_tokens.zip(output_tokens).map(|(i, o)| i + o)),
            prompt_cache_hit_tokens: usage
                .get("prompt_cache_hit_tokens")
                .and_then(Value::as_u64)
                .or(cached_tokens),
            prompt_cache_miss_tokens: usage
                .get("prompt_cache_miss_tokens")
                .and_then(Value::as_u64)
                .or_else(|| input_tokens.zip(cached_tokens).map(|(i, c)| i - c)),
        })
        .filter(|usage| {
            usage.prompt_tokens.is_some()
                || usage.completion_tokens.is_some()
                || usage.total_tokens.is_some()
                || usage.prompt_cache_hit_tokens.is_some()
                || usage.prompt_cache_miss_tokens.is_some()
        })
    }
}
