use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum RouterError {
    #[error("provider '{0}' is not available")]
    ProviderUnavailable(String),

    #[error("HTTP request failed: {0}")]
    HttpError(#[from] reqwest::Error),

    #[error("JSON parsing failed: {0}")]
    JsonError(#[from] serde_json::Error),

    #[error("missing API key: env var '{0}' is not set")]
    MissingApiKey(String),

    #[error("no provider registered for model '{0}'")]
    NoProviderForModel(String),

    #[error("all providers failed for model '{0}'")]
    AllProvidersFailed(String),

    #[error("{0}")]
    Internal(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComputeResources {
    pub gpu_type: Option<String>,
    pub gpu_name: Option<String>,
    /// Dedicated GPU memory as the driver reports it; none where the GPU
    /// shares system memory (Apple Silicon) or no driver reported a size.
    pub vram_gb: Option<f64>,
    pub ram_gb: f64,
    pub cpu_cores: usize,
    pub has_cuda: bool,
    pub has_metal: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: String,
    /// Either a plain string (text) or an OpenAI-style content array with
    /// `{"type":"text"|"image_url", ...}` parts. Typed as Value so multimodal
    /// callers (e.g. vision models on Featherless) pass through unchanged.
    #[serde(default = "Message::default_content")]
    pub content: Value,
    /// For tool role messages: the ID of the tool call being responded to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    /// For tool role messages: the tool function name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// For assistant messages with tool calls.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<serde_json::Value>>,
}

impl Message {
    fn default_content() -> Value {
        Value::String(String::new())
    }

    /// Best-effort extraction of the human-readable text. For plain string
    /// content returns it verbatim; for OpenAI array-shape content returns
    /// the concatenation of each `{type:"text",text:...}` part's text.
    pub fn content_text(&self) -> String {
        match &self.content {
            Value::String(s) => s.clone(),
            Value::Array(parts) => parts
                .iter()
                .filter_map(|p| {
                    if p.get("type").and_then(|v| v.as_str()) == Some("text") {
                        p.get("text").and_then(|v| v.as_str()).map(String::from)
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>()
                .join(""),
            _ => String::new(),
        }
    }
}

/// Tool definition following OpenAI function calling spec.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tool {
    #[serde(rename = "type")]
    pub tool_type: String,
    pub function: ToolFunction,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolFunction {
    pub name: String,
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parameters: Option<serde_json::Value>,
}

/// Tool call made by the model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub call_type: String,
    pub function: ToolCallFunction,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallFunction {
    pub name: String,
    pub arguments: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BillingTarget {
    pub provider_id: String,
    pub account_id: String,
    pub subscription_id: String,
}

/// A JSON Schema an answer must keep, under the name a provider's structured
/// output asks for.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResponseSchema {
    pub name: String,
    pub schema: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelRequest {
    pub messages: Vec<Message>,
    pub model: String,
    /// Sent to the provider only when the caller set one; otherwise the
    /// model's own output limit applies. A default the gateway invents would
    /// cut every answer of every caller that never asked for a cut.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    /// Sent to the provider only when the caller set one. A default the
    /// gateway invents reaches every provider as a real setting, and a model
    /// that refuses it ("`temperature` is deprecated for this model") refuses
    /// callers that never asked for it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<Tool>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub billing_target: Option<BillingTarget>,
    /// The shape the answer must keep, for a provider whose wire can hold its
    /// decoding to one. A decision sets it, so a text model cannot write a
    /// label its question never declared; a wire that has no such setting
    /// sends the request without it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_schema: Option<ResponseSchema>,
}

/// One window of a subscription's plan, as the provider itself reported it on
/// the response to a call Brama already made.
///
/// Brama does not model plan sizes. A provider that publishes utilization does
/// so as a fraction of its own limit and names when that window resets, and
/// that pair is the only honest thing to store: a token count Brama derived
/// would be a second, disagreeing account of a quota it does not own.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LimitReading {
    pub limit_id: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_label: Option<String>,
    pub used_fraction: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resets_at_ms: Option<i64>,
    /// When Brama read this window off a provider answer, or `None` when that
    /// was not recorded.
    ///
    /// A fraction without an instant cannot be aged: "100% used" is a different
    /// fact five minutes and five days after the provider said it, and a reader
    /// that cannot tell them apart shows a stale window as the current one.
    /// Ledgers written before this field existed carry none, and ledgers
    /// written while it was a plain integer carry zero for "not recorded";
    /// both read as `None` ([`unrecorded_as_none`]). A reading without one is
    /// reported as unrecorded and read as stale, never dated.
    #[serde(
        default,
        deserialize_with = "unrecorded_as_none",
        skip_serializing_if = "Option::is_none"
    )]
    pub recorded_at_ms: Option<i64>,
}

/// A reading's stored instant: absent, or the zero an earlier ledger format
/// wrote for "not recorded", is `None`; any other value is the instant.
fn unrecorded_as_none<'de, D>(deserializer: D) -> Result<Option<i64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(Option::<i64>::deserialize(deserializer)?
        .and_then(std::num::NonZeroI64::new)
        .map(std::num::NonZeroI64::get))
}

mod declared_age;
mod refusal;
mod response;
pub use declared_age::{declared_age, still_fresh};
pub use refusal::{GatewayRefusal, ProviderRefusal, Refusal};
pub use response::ModelResponse;
