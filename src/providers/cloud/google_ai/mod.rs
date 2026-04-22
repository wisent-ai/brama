use std::collections::HashMap;
use std::env;
use std::time::Instant;

use async_trait::async_trait;
use reqwest::Client;
use serde_json::{json, Value};
use tracing::{debug, error};

use crate::provider::ModelProvider;
use crate::types::{ModelRequest, ModelResponse};

fn alias_map() -> HashMap<&'static str, &'static str> {
    HashMap::from([
        ("gemini-2.0-flash", "gemini-2.0-flash-exp"),
        ("gemini-1.5-flash", "gemini-1.5-flash"),
    ])
}

pub struct GoogleAiProvider {
    client: Client,
    api_key: Option<String>,
}

impl GoogleAiProvider {
    pub fn new() -> Self {
        Self {
            client: Client::new(),
            api_key: env::var("GOOGLE_AI_API_KEY").ok(),
        }
    }

    fn supported_models() -> Vec<String> {
        alias_map().keys().map(|s| (*s).into()).collect()
    }
}

impl Default for GoogleAiProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ModelProvider for GoogleAiProvider {
    async fn complete(&self, request: &ModelRequest) -> ModelResponse {
        let api_key = match &self.api_key {
            Some(key) => key,
            None => {
                return ModelResponse::failure(
                    &request.model,
                    "GOOGLE_AI_API_KEY not set".into(),
                );
            }
        };

        let contents: Vec<Value> = request
            .messages
            .iter()
            .map(|m| {
                let role = if m.role == "assistant" { "model" } else { "user" };
                json!({
                    "role": role,
                    "parts": [{"text": m.content}],
                })
            })
            .collect();

        let upstream = alias_map()
            .get(request.model.as_str())
            .copied()
            .unwrap_or(request.model.as_str());

        let url = format!(
            "https://generativelanguage.googleapis.com/v1beta/models/{upstream}:generateContent?key={api_key}"
        );

        let mut body = json!({
            "contents": contents,
            "generationConfig": {
                "maxOutputTokens": request.max_tokens,
                "temperature": request.temperature,
            },
        });
        if let Some(system) = &request.system {
            body["systemInstruction"] = json!({
                "parts": [{"text": system}],
            });
        }

        debug!("Google AI request to model {}", upstream);
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
                error!("Google AI HTTP error: {e}");
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
            error!("Google AI error {status}: {body_text}");
            return ModelResponse::failure(
                &request.model,
                format!("API error {status}: {body_text}"),
            );
        }

        let parsed: Value = match serde_json::from_str(&body_text) {
            Ok(v) => v,
            Err(e) => {
                return ModelResponse::failure(
                    &request.model,
                    format!("JSON parse error: {e}"),
                );
            }
        };

        let content = parsed["candidates"][0]["content"]["parts"][0]["text"]
            .as_str()
            .unwrap_or("")
            .to_string();

        let input_tokens = parsed["usageMetadata"]["promptTokenCount"]
            .as_u64()
            .unwrap_or(0) as u32;
        let output_tokens = parsed["usageMetadata"]["candidatesTokenCount"]
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
            tool_calls: None,
        }
    }

    fn estimate_cost(&self, _input_tokens: u32, _output_tokens: u32) -> f64 {
        0.0
    }

    async fn is_available(&self) -> bool {
        self.api_key.is_some()
    }

    fn name(&self) -> &str {
        "google_ai"
    }

    fn models(&self) -> Vec<String> {
        Self::supported_models()
    }
}
