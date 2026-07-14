use std::collections::HashMap;
use std::env;
use std::time::Instant;

use async_trait::async_trait;
use reqwest::Client;
use serde_json::{json, Value};
use tracing::{debug, error};

use crate::gateway::broker::{provider_capability_configured, provider_credential};
use crate::provider::ModelProvider;
use crate::types::{ModelRequest, ModelResponse};

fn alias_map() -> HashMap<&'static str, &'static str> {
    HashMap::from([
        ("cf-llama-3.1-8b", "@cf/meta/llama-3.1-8b-instruct"),
        ("cf-mistral-7b", "@cf/mistral/mistral-7b-instruct-v0.1"),
    ])
}

pub struct CloudflareProvider {
    client: Client,
    account_id: Option<String>,
}

impl CloudflareProvider {
    pub fn new() -> Self {
        Self {
            client: Client::new(),
            account_id: env::var("CLOUDFLARE_ACCOUNT_ID").ok(),
        }
    }

    fn supported_models() -> Vec<String> {
        alias_map().keys().map(|s| (*s).into()).collect()
    }
}

impl Default for CloudflareProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ModelProvider for CloudflareProvider {
    async fn complete(&self, request: &ModelRequest) -> ModelResponse {
        let account_id = match &self.account_id {
            Some(account_id) => account_id,
            None => {
                return ModelResponse::failure(
                    &request.model,
                    "CLOUDFLARE_ACCOUNT_ID required".into(),
                );
            }
        };

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

        let url =
            format!("https://api.cloudflare.com/client/v4/accounts/{account_id}/ai/run/{upstream}");

        let body = json!({
            "messages": messages,
            "max_tokens": request.max_tokens,
        });

        debug!("Cloudflare request to model {}", upstream);
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
        let api_token = match credential.expose_utf8() {
            Ok(api_token) => api_token,
            Err(_) => {
                return ModelResponse::failure(
                    &request.model,
                    "Provider credential unavailable".into(),
                );
            }
        };

        let resp = self
            .client
            .post(&url)
            .bearer_auth(api_token)
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await;

        let elapsed = start.elapsed().as_secs_f64() * 1000.0;

        let resp = match resp {
            Ok(r) => r,
            Err(e) => {
                error!("Cloudflare HTTP error: {e}");
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
            error!("Cloudflare API error {status}: {body_text}");
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

        let content = parsed["result"]["response"]
            .as_str()
            .unwrap_or("")
            .to_string();

        ModelResponse {
            content,
            model: request.model.clone(),
            input_tokens: 0,
            output_tokens: 0,
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
        self.account_id.is_some() && provider_capability_configured(self.name())
    }

    fn name(&self) -> &str {
        "cloudflare"
    }

    fn models(&self) -> Vec<String> {
        Self::supported_models()
    }
}
