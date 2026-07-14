use std::time::Instant;

use async_trait::async_trait;
use reqwest::Client;
use serde_json::{json, Value};
use tracing::{debug, error};

use crate::gateway::broker::{provider_capability_configured, provider_credential};
use crate::provider::ModelProvider;
use crate::types::{ModelRequest, ModelResponse};

const API_URL: &str = "https://api.moonshot.cn/v1/chat/completions";

pub struct MoonshotProvider {
    client: Client,
}

impl MoonshotProvider {
    pub fn new() -> Self {
        Self {
            client: Client::new(),
        }
    }

    fn supported_models() -> Vec<String> {
        vec!["kimi-2.5".into()]
    }
}

impl Default for MoonshotProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ModelProvider for MoonshotProvider {
    async fn complete(&self, request: &ModelRequest) -> ModelResponse {
        let mut messages: Vec<Value> = Vec::new();
        if let Some(system) = &request.system {
            messages.push(json!({
                "role": "system",
                "content": system,
            }));
        }
        for m in &request.messages {
            messages.push(json!({
                "role": m.role,
                "content": m.content,
            }));
        }

        let body = json!({
            "model": request.model,
            "max_tokens": request.max_tokens,
            "temperature": request.temperature,
            "messages": messages,
        });

        debug!("Moonshot request to model {}", request.model);
        let start = Instant::now();

        let credential = match provider_credential(self.name()).await {
            Some(credential) => credential,
            None => {
                return ModelResponse::failure(
                    &request.model,
                    "Provider credential unavailable".into(),
                );
            }
        };
        let api_key = match credential.expose_utf8() {
            Ok(api_key) => api_key,
            Err(_) => {
                return ModelResponse::failure(
                    &request.model,
                    "Provider credential unavailable".into(),
                );
            }
        };

        let resp = self
            .client
            .post(API_URL)
            .header("authorization", format!("Bearer {api_key}"))
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await;

        let elapsed = start.elapsed().as_secs_f64() * 1000.0;

        let resp = match resp {
            Ok(r) => r,
            Err(e) => {
                error!("Moonshot HTTP error: {e}");
                return ModelResponse::failure(&request.model, format!("HTTP error: {e}"));
            }
        };

        let status = resp.status();
        let body_text = match resp.text().await {
            Ok(t) => t,
            Err(e) => {
                return ModelResponse::failure(&request.model, format!("read body failed: {e}"));
            }
        };

        if !status.is_success() {
            error!("Moonshot API error {status}: {body_text}");
            return ModelResponse::failure(
                &request.model,
                format!("API error {status}: {body_text}"),
            );
        }

        let parsed: Value = match serde_json::from_str(&body_text) {
            Ok(v) => v,
            Err(e) => {
                return ModelResponse::failure(&request.model, format!("JSON parse error: {e}"));
            }
        };

        let content = parsed["choices"]
            .as_array()
            .and_then(|arr| arr.first())
            .and_then(|c| c["message"]["content"].as_str())
            .unwrap_or("")
            .to_string();

        let input_tokens = parsed["usage"]["prompt_tokens"].as_u64().unwrap_or(0) as u32;
        let output_tokens = parsed["usage"]["completion_tokens"].as_u64().unwrap_or(0) as u32;

        let cost = self.estimate_cost(input_tokens, output_tokens);

        ModelResponse {
            content,
            model: request.model.clone(),
            input_tokens,
            output_tokens,
            latency_ms: elapsed,
            cost,
            success: true,
            error: None,
            tool_calls: None,
        }
    }

    fn estimate_cost(&self, input_tokens: u32, output_tokens: u32) -> f64 {
        let inp = 1.0;
        let out = 2.0;
        (input_tokens as f64 / 1_000_000.0) * inp + (output_tokens as f64 / 1_000_000.0) * out
    }

    async fn is_available(&self) -> bool {
        provider_capability_configured(self.name())
    }

    fn name(&self) -> &str {
        "moonshot"
    }

    fn models(&self) -> Vec<String> {
        Self::supported_models()
    }
}
