use std::env;
use std::time::Instant;

use async_trait::async_trait;
use reqwest::Client;
use serde_json::{json, Value};
use tracing::{debug, error};

use crate::provider::ModelProvider;
use crate::types::{
    ModelRequest, ModelResponse, ToolCall, ToolCallFunction,
};

const API_URL: &str =
    "https://api.featherless.ai/v1/chat/completions";

const DEFAULT_FEATHERLESS_MODEL: &str =
    "TheDrummer/Cydonia-24B-v4.3";

const ALIAS: &str = "cydonia-24b";

/// Alias → upstream HF path for all Featherless-registered models beyond
/// the env-configurable primary alias.
fn extra_aliases() -> &'static [(&'static str, &'static str)] {
    &[
        ("gemma-3-27b-abliterated", "mlabonne/gemma-3-27b-it-abliterated"),
    ]
}

pub struct FeatherlessProvider {
    client: Client,
    api_key: Option<String>,
    upstream_model: String,
}

impl FeatherlessProvider {
    pub fn new() -> Self {
        let upstream_model = env::var("FEATHERLESS_MODEL")
            .unwrap_or_else(|_| DEFAULT_FEATHERLESS_MODEL.into());
        Self {
            client: Client::new(),
            api_key: env::var("FEATHERLESS_API_KEY").ok(),
            upstream_model,
        }
    }

    fn supported_models() -> Vec<String> {
        let mut v = vec![ALIAS.into()];
        v.extend(extra_aliases().iter().map(|(a, _)| (*a).into()));
        v
    }
}

impl Default for FeatherlessProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ModelProvider for FeatherlessProvider {
    async fn complete(
        &self,
        request: &ModelRequest,
    ) -> ModelResponse {
        let api_key = match &self.api_key {
            Some(key) => key,
            None => {
                return ModelResponse::failure(
                    &request.model,
                    "FEATHERLESS_API_KEY not set".into(),
                );
            }
        };

        let mut messages: Vec<Value> = Vec::new();
        if let Some(system) = &request.system {
            messages.push(json!({
                "role": "system",
                "content": system,
            }));
        }
        for m in &request.messages {
            let mut msg = json!({"role": m.role, "content": m.content});
            if let Some(ref tc_id) = m.tool_call_id {
                msg["tool_call_id"] = json!(tc_id);
            }
            if let Some(ref name) = m.name {
                msg["name"] = json!(name);
            }
            if let Some(ref tcs) = m.tool_calls {
                msg["tool_calls"] = json!(tcs);
            }
            messages.push(msg);
        }

        let upstream: &str = if request.model == ALIAS {
            self.upstream_model.as_str()
        } else if let Some((_, up)) =
            extra_aliases().iter().find(|(a, _)| *a == request.model)
        {
            up
        } else {
            request.model.as_str()
        };

        let mut body = json!({
            "model": upstream,
            "max_tokens": request.max_tokens,
            "temperature": request.temperature,
            "messages": messages,
        });

        if let Some(tools) = &request.tools {
            if !tools.is_empty() {
                body["tools"] =
                    serde_json::to_value(tools).unwrap();
            }
        }

        debug!("Featherless request to model {}", upstream);
        let start = Instant::now();

        let resp = self
            .client
            .post(API_URL)
            .header(
                "authorization",
                format!("Bearer {api_key}"),
            )
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await;

        let elapsed = start.elapsed().as_secs_f64() * 1000.0;

        let resp = match resp {
            Ok(r) => r,
            Err(e) => {
                error!("Featherless HTTP error: {e}");
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
            error!("Featherless API error {status}: {body_text}");
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

        let choice = parsed["choices"]
            .as_array()
            .and_then(|arr| arr.first());

        let content = choice
            .and_then(|c| c["message"]["content"].as_str())
            .unwrap_or("")
            .to_string();

        let tool_calls = choice
            .and_then(|c| c["message"]["tool_calls"].as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|tc| {
                        Some(ToolCall {
                            id: tc["id"].as_str()?.to_string(),
                            call_type: "function".into(),
                            function: ToolCallFunction {
                                name: tc["function"]["name"]
                                    .as_str()?
                                    .to_string(),
                                arguments: tc["function"]
                                    ["arguments"]
                                    .as_str()?
                                    .to_string(),
                            },
                        })
                    })
                    .collect::<Vec<_>>()
            })
            .filter(|v| !v.is_empty());

        let input_tokens = parsed["usage"]["prompt_tokens"]
            .as_u64()
            .unwrap_or(0) as u32;
        let output_tokens =
            parsed["usage"]["completion_tokens"]
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
            tool_calls,
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
        self.api_key.is_some()
    }

    fn name(&self) -> &str {
        "featherless"
    }

    fn models(&self) -> Vec<String> {
        Self::supported_models()
    }
}
