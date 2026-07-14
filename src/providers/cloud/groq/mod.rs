use std::collections::HashMap;
use std::time::Instant;

use async_trait::async_trait;
use reqwest::Client;
use serde_json::{json, Value};
use tracing::{debug, error};

use crate::gateway::broker::{provider_capability_configured, provider_credential};
use crate::provider::ModelProvider;
use crate::types::{ModelRequest, ModelResponse};

const API_URL: &str = "https://api.groq.com/openai/v1/chat/completions";

fn alias_map() -> HashMap<&'static str, &'static str> {
    HashMap::from([
        ("groq-llama-3.3-70b", "llama-3.3-70b-versatile"),
        ("groq-llama-3.1-8b", "llama-3.1-8b-instant"),
        ("groq-mixtral", "mixtral-8x7b-32768"),
    ])
}

pub struct GroqProvider {
    client: Client,
}

impl GroqProvider {
    pub fn new() -> Self {
        Self {
            client: Client::new(),
        }
    }

    fn supported_models() -> Vec<String> {
        alias_map().keys().map(|s| (*s).into()).collect()
    }
}

impl Default for GroqProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ModelProvider for GroqProvider {
    async fn complete(&self, request: &ModelRequest) -> ModelResponse {

        let mut messages: Vec<Value> = Vec::new();
        if let Some(system) = &request.system {
            messages.push(json!({"role": "system", "content": system}));
        }
        for m in &request.messages {
            messages.push(json!({"role": m.role, "content": m.content}));
        }

        let upstream = alias_map()
            .get(request.model.as_str())
            .copied()
            .unwrap_or(request.model.as_str());

        let body = json!({
            "model": upstream,
            "max_tokens": request.max_tokens,
            "temperature": request.temperature,
            "messages": messages,
        });

        debug!("Groq request to model {}", upstream);
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
                error!("Groq HTTP error: {e}");
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
            error!("Groq API error {status}: {body_text}");
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

        let content = parsed["choices"][0]["message"]["content"]
            .as_str()
            .unwrap_or("")
            .to_string();
        let input_tokens = parsed["usage"]["prompt_tokens"].as_u64().unwrap_or(0) as u32;
        let output_tokens = parsed["usage"]["completion_tokens"].as_u64().unwrap_or(0) as u32;

        ModelResponse {
            content,
            model: request.model.clone(),
            input_tokens,
            output_tokens,
            latency_ms: elapsed,
            cost: 0.0,
            success: true,
            error: None,
            tool_calls: None,
        }
    }

    fn estimate_cost(&self, _input_tokens: u32, _output_tokens: u32) -> f64 {
        0.0
    }

    async fn is_available(&self) -> bool {
        provider_capability_configured(self.name())
    }

    fn name(&self) -> &str {
        "groq"
    }

    fn models(&self) -> Vec<String> {
        Self::supported_models()
    }
}
