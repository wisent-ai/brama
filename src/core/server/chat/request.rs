//! The chat-completions wire shapes, the selectors a model name may be instead
//! of a route, and the bounds every request in this gateway is held to.

use serde::{Deserialize, Serialize};

use crate::types::{BillingTarget, ResponseSchema, Tool, ToolCall};

/// A temperature runs over the range the chat-completions format defines. A
/// request names no answer length unless the caller does, and it runs until
/// the provider answers or refuses; no clock ends it.
pub(crate) const MIN_TEMPERATURE: f64 = 0.0; // https://platform.openai.com/docs/api-reference/chat/create#chat-create-temperature
pub(crate) const MAX_TEMPERATURE: f64 = 2.0; // https://platform.openai.com/docs/api-reference/chat/create#chat-create-temperature

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
    /// The shape the answer must take; see [`ResponseFormat`].
    #[serde(default)]
    pub(super) response_format: Option<ResponseFormat>,
}

/// The chat-completions `response_format`. Only `json_schema` is taken: it
/// names the schema the answer is held to, the same [`ResponseSchema`] the
/// decisions endpoint asks with. A provider that decodes under a schema is
/// sent it; every answer is refused unless it is one JSON object. `text`
/// asks for nothing, and `json_object` names no schema, so both are refused
/// by name rather than read as a schema.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ResponseFormat {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    json_schema: Option<JsonSchemaFormat>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct JsonSchemaFormat {
    name: String,
    schema: serde_json::Value,
    #[serde(default)]
    strict: Option<bool>,
}

impl ResponseFormat {
    /// The schema the answer is held to, or why this format names none.
    pub(super) fn schema(self) -> Result<ResponseSchema, String> {
        if self.kind != "json_schema" {
            return Err(format!(
                "response_format type `{}` is not taken: only `json_schema`, which names the schema the answer is held to",
                self.kind
            ));
        }
        let Some(format) = self.json_schema else {
            return Err("response_format `json_schema` carries no `json_schema` object".into());
        };
        if format.strict == Some(false) {
            return Err(
                "response_format `json_schema` with `strict: false` is not taken: Brama holds an answer to its schema strictly or not at all"
                    .into(),
            );
        }
        Ok(ResponseSchema {
            name: format.name,
            schema: format.schema,
        })
    }
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
