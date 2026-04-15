use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum RouterError {
    #[error("provider '{0}' is not available")]
    ProviderUnavailable(String),

    #[error("HTTP request failed: {0}")]
    HttpError(#[from] reqwest::Error),

    #[error("JSON parsing failed: {0}")]
    JsonError(#[from] serde_json::Error),

    #[error(
        "missing API key: env var '{0}' is not set"
    )]
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
    #[serde(default)]
    pub content: String,
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
    #[serde(
        default,
        skip_serializing_if = "Option::is_none"
    )]
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelRequest {
    pub messages: Vec<Message>,
    pub model: String,
    pub max_tokens: u32,
    pub temperature: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<Tool>>,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
}

impl ModelResponse {
    pub fn failure(model: &str, error: String) -> Self {
        Self {
            content: String::new(),
            model: model.to_string(),
            input_tokens: 0,
            output_tokens: 0,
            latency_ms: 0.0,
            cost: 0.0,
            success: false,
            error: Some(error),
            tool_calls: None,
        }
    }
}
