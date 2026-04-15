use std::env;
use std::time::Instant;

use async_trait::async_trait;
use reqwest::Client;
use serde_json::{json, Value};
use tracing::{debug, error};

use crate::provider::ModelProvider;
use crate::types::{ModelRequest, ModelResponse};

const API_URL: &str = "https://api.anthropic.com/v1/messages";
const API_VERSION: &str = "2023-06-01";

pub struct AnthropicProvider {
    client: Client,
    api_key: Option<String>,
}

impl AnthropicProvider {
    pub fn new() -> Self {
        Self {
            client: Client::new(),
            api_key: env::var("ANTHROPIC_API_KEY").ok(),
        }
    }

    fn supported_models() -> Vec<String> {
        vec![
            "claude-opus-4-5".into(),
            "claude-opus-4".into(),
            "claude-sonnet-4".into(),
            "claude-haiku-3.5".into(),
        ]
    }
}

impl Default for AnthropicProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ModelProvider for AnthropicProvider {
    async fn complete(
        &self,
        request: &ModelRequest,
    ) -> ModelResponse {
        let api_key = match &self.api_key {
            Some(key) => key,
            None => {
                return ModelResponse::failure(
                    &request.model,
                    "ANTHROPIC_API_KEY not set".into(),
                );
            }
        };

        let messages: Vec<Value> = request
            .messages
            .iter()
            .map(|m| {
                json!({
                    "role": m.role,
                    "content": m.content,
                })
            })
            .collect();

        let mut body = json!({
            "model": request.model,
            "max_tokens": request.max_tokens,
            "temperature": request.temperature,
            "messages": messages,
        });

        if let Some(system) = &request.system {
            body["system"] = json!(system);
        }

        debug!("Anthropic request to model {}", request.model);
        let start = Instant::now();

        let resp = self
            .client
            .post(API_URL)
            .header("x-api-key", api_key)
            .header("anthropic-version", API_VERSION)
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await;

        let elapsed = start.elapsed().as_secs_f64() * 1000.0;

        let resp = match resp {
            Ok(r) => r,
            Err(e) => {
                error!("Anthropic HTTP error: {e}");
                return ModelResponse::failure(
                    &request.model,
                    format!("HTTP error: {e}"),
                );
            }
        };

        let status = resp.status();
        let body_text = match resp.text().await {
            Ok(t) => t,
            Err(e) => {
                return ModelResponse::failure(
                    &request.model,
                    format!("read body failed: {e}"),
                );
            }
        };

        if !status.is_success() {
            error!("Anthropic API error {status}: {body_text}");
            return ModelResponse::failure(
                &request.model,
                format!("API error {status}: {body_text}"),
            );
        }

        let parsed: Value = match serde_json::from_str(&body_text)
        {
            Ok(v) => v,
            Err(e) => {
                return ModelResponse::failure(
                    &request.model,
                    format!("JSON parse error: {e}"),
                );
            }
        };

        let content = parsed["content"]
            .as_array()
            .and_then(|arr| arr.first())
            .and_then(|block| block["text"].as_str())
            .unwrap_or("")
            .to_string();

        let input_tokens = parsed["usage"]["input_tokens"]
            .as_u64()
            .unwrap_or(0) as u32;
        let output_tokens = parsed["usage"]["output_tokens"]
            .as_u64()
            .unwrap_or(0) as u32;

        let cost =
            self.estimate_cost(input_tokens, output_tokens);

        ModelResponse {
            content,
            model: request.model.clone(),
            input_tokens,
            output_tokens,
            latency_ms: elapsed,
            cost,
            success: true,
            error: None,
        }
    }

    fn estimate_cost(
        &self,
        input_tokens: u32,
        output_tokens: u32,
    ) -> f64 {
        let inp = 15.0;
        let out = 75.0;
        (input_tokens as f64 / 1_000_000.0) * inp
            + (output_tokens as f64 / 1_000_000.0) * out
    }

    async fn is_available(&self) -> bool {
        self.api_key.is_some()
    }

    fn name(&self) -> &str {
        "anthropic"
    }

    fn models(&self) -> Vec<String> {
        Self::supported_models()
    }
}
