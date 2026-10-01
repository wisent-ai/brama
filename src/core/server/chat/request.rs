//! The chat-completions wire shapes, the selectors a model name may be instead
//! of a route, and the bounds every request in this gateway is held to.

use serde::{Deserialize, Serialize};

use crate::types::{BillingTarget, Tool, ToolCall};

/// A temperature runs from zero to 2, the range the chat-completions format
/// defines. A request names no answer length unless the caller does, and it
/// runs until the provider answers or refuses; no clock ends it.
pub(crate) const MAX_TEMPERATURE: f64 = 2.0;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ChatCompletionRequest {
    #[serde(default)]
    pub(super) model: Option<String>,
    pub(super) messages: Vec<ChatMessage>,
    #[serde(default)]
    pub(super) max_tokens: Option<u32>,
    #[serde(default)]
    pub(super) temperature: Option<f64>,
    #[serde(default)]
    pub(super) tools: Option<Vec<Tool>>,
    #[serde(default)]
    pub(super) tool_choice: Option<serde_json::Value>,
    #[serde(default, rename = "billingTarget")]
    pub(super) billing_target: Option<BillingTarget>,
    /// Ask for server-sent events instead of one buffered completion.
    #[serde(default)]
    pub(super) stream: bool,
}

pub(super) fn is_any_subscription_selector(model: &str) -> bool {
    model.trim().eq_ignore_ascii_case("any")
}

pub(super) fn is_any_vision_capable_subscription_selector(model: &str) -> bool {
    model.trim().eq_ignore_ascii_case("any-vision-capable")
}

pub(super) fn task_subscription_selector(model: &str) -> Option<String> {
    model
        .trim()
        .strip_prefix("task:")
        .map(str::trim)
        .filter(|task| !task.is_empty())
        .map(String::from)
}

#[derive(Debug, Deserialize)]
pub(super) struct ChatMessage {
    pub(super) role: String,
    #[serde(default)]
    pub(super) content: Option<serde_json::Value>,
    #[serde(default)]
    pub(super) tool_call_id: Option<String>,
    #[serde(default)]
    pub(super) name: Option<String>,
    #[serde(default)]
    pub(super) tool_calls: Option<Vec<serde_json::Value>>,
}

#[derive(Debug, Serialize)]
pub(super) struct ChatCompletionResponse {
    pub(super) id: String,
    pub(super) object: String,
    pub(super) model: String,
    pub(super) choices: Vec<Choice>,
    pub(super) usage: Usage,
}

#[derive(Debug, Serialize)]
pub(super) struct Choice {
    pub(super) index: u32,
    pub(super) message: ChoiceMessage,
    pub(super) finish_reason: String,
}

#[derive(Debug, Serialize)]
pub(super) struct ChoiceMessage {
    pub(super) role: String,
    pub(super) content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) tool_calls: Option<Vec<ToolCall>>,
}

#[derive(Debug, Serialize)]
pub(super) struct Usage {
    pub(super) prompt_tokens: u32,

    pub(super) completion_tokens: u32,
    pub(super) total_tokens: u32,
}
