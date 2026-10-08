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
    pub vram_gb: f64,
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
    /// When Brama read this window off a provider answer.
    ///
    /// A fraction without an instant cannot be aged: "100% used" is a different
    /// fact five minutes and five days after the provider said it, and a reader
    /// that cannot tell them apart shows a stale window as the current one.
    /// Absent in ledgers written before this field existed, which deserialize
    /// to zero and are backfilled from the ledger file's own timestamp.
    #[serde(default)]
    pub recorded_at_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelResponse {
    pub content: String,
    pub model: String,
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub latency_ms: f64,
    pub cost: f64,
    pub success: bool,
    /// Number of provider HTTP calls consumed by this routed request.
    #[serde(default)]
    pub attempts: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
    /// Plan windows the provider reported on this exact response, when it
    /// reports any. Present on rate-limited responses too, which is when it
    /// matters most.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub limits: Vec<LimitReading>,
    /// The class of a refusal, stated where it was built: from the provider's
    /// HTTP status where one answered, and from Brama's own reason where Brama
    /// refused. Rotation and the HTTP edge decide from this, never from the
    /// sentence, which is data and changes wording without notice. Absent only
    /// on a success.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_kind: Option<ProviderRefusal>,
}

impl ModelResponse {
    /// A failed answer and its class. There is no failure without one: the
    /// edge answers from the class, and a refusal whose class nobody stated
    /// would have to be guessed from its sentence.
    pub fn failure(model: &str, class: ProviderRefusal, error: String) -> Self {
        Self {
            content: String::new(),
            model: model.to_string(),
            input_tokens: 0,
            output_tokens: 0,
            latency_ms: 0.0,
            cost: 0.0,
            attempts: 0,
            success: false,
            error: Some(error),
            tool_calls: None,
            limits: Vec::new(),
            failure_kind: Some(class),
        }
    }

    /// A refusal Brama builds itself, with its class stated.
    pub fn refused(model: &str, class: GatewayRefusal, error: String) -> Self {
        Self::failure(model, ProviderRefusal::Gateway(class), error)
    }

    /// A failed answer carrying one stated refusal.
    pub fn from_refusal(model: &str, refusal: Refusal) -> Self {
        Self::failure(model, refusal.class, refusal.message)
    }
}

mod declared_age;
mod refusal;
pub use declared_age::{declared_age, still_fresh};
pub use refusal::{GatewayRefusal, ProviderRefusal, Refusal};
