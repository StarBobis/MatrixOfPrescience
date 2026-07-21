use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use crate::*;

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ChatMessage {
    pub(crate) role: String,
    pub(crate) content: String,
    #[serde(default, alias = "reasoning_content")]
    pub(crate) reasoning_content: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ChatCompletionRequest {
    pub(crate) provider_name: String,
    pub(crate) base_url: String,
    pub(crate) api_key: String,
    pub(crate) model: String,
    pub(crate) wire_api: Option<String>,
    pub(crate) reasoning_effort: Option<String>,
    pub(crate) temperature: Option<f32>,
    pub(crate) system_prompt: Option<String>,
    pub(crate) workspace_path: Option<String>,
    pub(crate) code_tools_enabled: Option<bool>,
    pub(crate) orchestration_tools_enabled: Option<bool>,
    pub(crate) orchestration_required: Option<bool>,
    pub(crate) can_write: Option<bool>,
    pub(crate) stream_id: Option<String>,
    pub(crate) cancellation_id: Option<String>,
    pub(crate) messages: Vec<ChatMessage>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ChatCompletionResponse {
    pub(crate) content: String,
    pub(crate) trace_steps: Vec<ChatTraceStep>,
    pub(crate) usage: Option<ChatCompletionUsage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) dispatched_tasks: Option<Vec<TaskDispatchedEntry>>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TaskDispatchedEntry {
    pub(crate) member: String,
    pub(crate) instruction: String,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ChatCompletionStreamEvent {
    pub(crate) stream_id: String,
    pub(crate) event_type: String,
    pub(crate) trace_kind: Option<String>,
    pub(crate) text: String,
    pub(crate) detail: Option<String>,
    pub(crate) usage: Option<ChatCompletionUsage>,
    pub(crate) retry_attempt: Option<usize>,
    pub(crate) retry_delay_ms: Option<u64>,
    pub(crate) retry_reason: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum HttpRetryProgress {
    Waiting {
        attempt: usize,
        delay: Duration,
        reason: String,
    },
    Recovered {
        attempts: usize,
    },
}

#[derive(Debug, Default)]
pub(crate) struct ToolCallAccumulator {
    pub(crate) id: String,
    pub(crate) call_type: String,
    pub(crate) function_name: String,
    pub(crate) function_arguments: String,
}

#[derive(Default)]
pub(crate) struct ChatCancellationState {
    pub(crate) tokens: Mutex<HashMap<String, Arc<AtomicBool>>>,
}

impl ChatCancellationState {
    pub(crate) fn token(&self, cancellation_id: &str) -> Arc<AtomicBool> {
        let mut tokens = self
            .tokens
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        tokens
            .entry(cancellation_id.to_string())
            .or_insert_with(|| Arc::new(AtomicBool::new(false)))
            .clone()
    }

    pub(crate) fn cancel(&self, cancellation_id: &str) {
        let token = self.token(cancellation_id);
        token.store(true, Ordering::Release);
    }

    pub(crate) fn finish(&self, cancellation_id: &str) {
        self.tokens
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(cancellation_id);
    }
}

