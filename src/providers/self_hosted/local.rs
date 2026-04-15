use std::env;
use std::time::Instant;

use async_trait::async_trait;
use reqwest::Client;
use serde_json::{json, Value};
use tracing::{debug, error, warn};

use crate::provider::ModelProvider;
use crate::types::{ModelRequest, ModelResponse};

const DEFAULT_ENDPOINT: &str = "http://localhost:8000";

pub struct LocalProvider {
    client: Client,
    endpoint: String,
}

impl LocalProvider {
    pub fn new() -> Self {
        let endpoint = env::var("LLAMA_ENDPOINT")
            .unwrap_or_else(|_| DEFAULT_ENDPOINT.into());
        Self {
            client: Client::new(),
            endpoint,
        }
    }

    fn completions_url(&self) -> String {
        format!("{}/v1/chat/completions", self.endpoint)
    }

    fn supported_models() -> Vec<String> {
        vec![
            "deepseek-r1-qwen3-8b".into(),
            "qwen3-8b".into(),
            "qwen3-4b".into(),
        ]
    }
}

impl Default for LocalProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ModelProvider for LocalProvider {
    async fn complete(
        &self,
        request: &ModelRequest,
    ) -> ModelResponse {
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

        let url = self.completions_url();
        debug!(
            "Local request model={} endpoint={}",
            request.model, url
        );
        let start = Instant::now();

        let resp = self
            .client
            .post(&url)
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await;

        let elapsed = start.elapsed().as_secs_f64() * 1000.0;

        let resp = match resp {
            Ok(r) => r,
            Err(e) => {
                error!("Local endpoint error: {e}");
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
            error!("Local API error {status}: {body_text}");
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

        let content = parsed["choices"]
            .as_array()
            .and_then(|arr| arr.first())
            .and_then(|c| c["message"]["content"].as_str())
            .unwrap_or("")
            .to_string();

        let input_tokens = parsed["usage"]["prompt_tokens"]
            .as_u64()
            .unwrap_or(0) as u32;
        let output_tokens =
            parsed["usage"]["completion_tokens"]
                .as_u64()
                .unwrap_or(0) as u32;

        ModelResponse {
            content,
            model: request.model.clone(),
            input_tokens,
            output_tokens,
            latency_ms: elapsed,
            cost: 0.0,
            success: true,
            error: None,
        }
    }

    fn estimate_cost(
        &self,
        _input_tokens: u32,
        _output_tokens: u32,
    ) -> f64 {
        0.0
    }

    async fn is_available(&self) -> bool {
        let url = format!("{}/health", self.endpoint);
        match self.client.get(&url).send().await {
            Ok(resp) => resp.status().is_success(),
            Err(e) => {
                warn!(
                    "Local endpoint unavailable at {}: {e}",
                    self.endpoint
                );
                false
            }
        }
    }

    fn name(&self) -> &str {
        "local"
    }

    fn models(&self) -> Vec<String> {
        Self::supported_models()
    }
}
