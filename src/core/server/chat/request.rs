//! The chat-completions wire shapes, the selectors a model name may be instead
//! of a route, and the three limits every request in this gateway is held to.

use serde::{Deserialize, Serialize};

use crate::types::{BillingTarget, Tool, ToolCall};

pub(in crate::core::server) fn max_output_tokens() -> u32 {
    "32768".parse().expect("valid output token limit")
}

pub(super) fn max_temperature() -> f64 {
    "2".parse().expect("valid temperature limit")
}

pub(in crate::core::server) fn request_deadline() -> std::time::Duration {
    std::time::Duration::from_secs("300".parse().expect("valid request deadline"))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ChatCompletionRequest {
    #[serde(default)]
    pub(super) model: Option<String>,
    pub(super) messages: Vec<ChatMessage>,
    #[serde(default = "default_max_tokens")]
    pub(super) max_tokens: u32,
    #[serde(default = "default_temperature")]
    pub(super) temperature: f64,
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

fn default_max_tokens() -> u32 {
    1024
}
fn default_temperature() -> f64 {
    0.7
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
