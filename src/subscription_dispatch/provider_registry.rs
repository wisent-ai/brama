//! Native Wisent provider registry for credentials redeemed from Skarbiec.
//!
//! This module is intentionally independent from external agent harnesses. It owns
//! provider discovery, request shaping and response normalization for API-backed
//! subscriptions. Secrets are passed in-memory by the Skarbiec capability broker
//! and are never persisted here.

use std::time::Instant;

use reqwest::{Client, RequestBuilder};
use serde_json::{json, Map, Value};

use crate::types::{Message, ModelRequest, ModelResponse, ToolCall};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WireProtocol {
    OpenAiChat,
    AnthropicMessages,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthKind {
    Bearer,
    XApiKey,
}

#[derive(Clone, Copy, Debug)]
pub struct ProviderDescriptor {
    pub id: &'static str,
    pub display_name: &'static str,
    pub base_url: &'static str,
    pub models_path: &'static str,
    pub chat_path: &'static str,
    pub wire: WireProtocol,
    pub auth: AuthKind,
    pub static_models: &'static [&'static str],
}

#[derive(Clone, Debug)]
pub struct RegistryModel {
    pub route_id: String,
    pub provider_id: String,
    pub model_id: String,
    pub context_window: u64,
    pub max_output_tokens: u64,
    pub input_modalities: Vec<String>,
    pub tools: bool,
    pub reasoning: bool,
}

const PROVIDERS: &[ProviderDescriptor] = &[
    ProviderDescriptor {
        id: "anthropic",
        display_name: "Anthropic",
        base_url: "https://api.anthropic.com",
        models_path: "/v1/models",
        chat_path: "/v1/messages",
        wire: WireProtocol::AnthropicMessages,
        auth: AuthKind::XApiKey,
        static_models: &["claude-haiku-4-5", "claude-opus-4-6", "claude-sonnet-4-6"],
    },
    ProviderDescriptor {
        id: "openai",
        display_name: "OpenAI",
        base_url: "https://api.openai.com",
        models_path: "/v1/models",
        chat_path: "/v1/chat/completions",
        wire: WireProtocol::OpenAiChat,
        auth: AuthKind::Bearer,
        static_models: &[],
    },
    ProviderDescriptor {
        id: "openrouter",
        display_name: "OpenRouter",
        base_url: "https://openrouter.ai/api",
        models_path: "/v1/models",
        chat_path: "/v1/chat/completions",
        wire: WireProtocol::OpenAiChat,
        auth: AuthKind::Bearer,
        static_models: &[],
    },
    ProviderDescriptor {
        id: "groq",
        display_name: "Groq",
        base_url: "https://api.groq.com/openai",
        models_path: "/v1/models",
        chat_path: "/v1/chat/completions",
        wire: WireProtocol::OpenAiChat,
        auth: AuthKind::Bearer,
        static_models: &[],
    },
    ProviderDescriptor {
        id: "mistral",
        display_name: "Mistral",
        base_url: "https://api.mistral.ai",
        models_path: "/v1/models",
        chat_path: "/v1/chat/completions",
        wire: WireProtocol::OpenAiChat,
        auth: AuthKind::Bearer,
        static_models: &[],
    },
    ProviderDescriptor {
        id: "xai",
        display_name: "xAI",
        base_url: "https://api.x.ai",
        models_path: "/v1/models",
        chat_path: "/v1/chat/completions",
        wire: WireProtocol::OpenAiChat,
        auth: AuthKind::Bearer,
        static_models: &[],
    },
    ProviderDescriptor {
        id: "deepseek",
        display_name: "DeepSeek",
        base_url: "https://api.deepseek.com",
        models_path: "/models",
        chat_path: "/chat/completions",
        wire: WireProtocol::OpenAiChat,
        auth: AuthKind::Bearer,
        static_models: &["deepseek-chat", "deepseek-reasoner"],
    },
    ProviderDescriptor {
        id: "cerebras",
        display_name: "Cerebras",
        base_url: "https://api.cerebras.ai",
        models_path: "/v1/models",
        chat_path: "/v1/chat/completions",
        wire: WireProtocol::OpenAiChat,
        auth: AuthKind::Bearer,
        static_models: &[],
    },
    ProviderDescriptor {
        id: "fireworks",
        display_name: "Fireworks",
        base_url: "https://api.fireworks.ai/inference",
        models_path: "/v1/models",
        chat_path: "/v1/chat/completions",
        wire: WireProtocol::OpenAiChat,
        auth: AuthKind::Bearer,
        static_models: &[],
    },
    ProviderDescriptor {
        id: "together",
        display_name: "Together",
        base_url: "https://api.together.xyz",
        models_path: "/v1/models",
        chat_path: "/v1/chat/completions",
        wire: WireProtocol::OpenAiChat,
        auth: AuthKind::Bearer,
        static_models: &[],
    },
    ProviderDescriptor {
        id: "nvidia",
        display_name: "NVIDIA NIM",
        base_url: "https://integrate.api.nvidia.com",
        models_path: "/v1/models",
        chat_path: "/v1/chat/completions",
        wire: WireProtocol::OpenAiChat,
        auth: AuthKind::Bearer,
        static_models: &[],
    },
    ProviderDescriptor {
        id: "moonshot",
        display_name: "Moonshot",
        base_url: "https://api.moonshot.ai",
        models_path: "/v1/models",
        chat_path: "/v1/chat/completions",
        wire: WireProtocol::OpenAiChat,
        auth: AuthKind::Bearer,
        static_models: &[],
    },
    ProviderDescriptor {
        id: "zai",
        display_name: "Z.AI",
        base_url: "https://api.z.ai/api/paas",
        models_path: "/v4/models",
        chat_path: "/v4/chat/completions",
        wire: WireProtocol::OpenAiChat,
        auth: AuthKind::Bearer,
        static_models: &[],
    },
    ProviderDescriptor {
        id: "qwen",
        display_name: "Qwen",
        base_url: "https://dashscope-intl.aliyuncs.com/compatible-mode",
        models_path: "/v1/models",
        chat_path: "/v1/chat/completions",
        wire: WireProtocol::OpenAiChat,
        auth: AuthKind::Bearer,
        static_models: &[],
    },
    ProviderDescriptor {
        id: "huggingface",
        display_name: "Hugging Face Inference",
        base_url: "https://router.huggingface.co",
        models_path: "/v1/models",
        chat_path: "/v1/chat/completions",
        wire: WireProtocol::OpenAiChat,
        auth: AuthKind::Bearer,
        static_models: &[],
    },
    ProviderDescriptor {
        id: "venice",
        display_name: "Venice",
        base_url: "https://api.venice.ai/api",
        models_path: "/v1/models",
        chat_path: "/v1/chat/completions",
        wire: WireProtocol::OpenAiChat,
        auth: AuthKind::Bearer,
        static_models: &[],
    },
    ProviderDescriptor {
        id: "novita",
        display_name: "Novita",
        base_url: "https://api.novita.ai/openai",
        models_path: "/v1/models",
        chat_path: "/v1/chat/completions",
        wire: WireProtocol::OpenAiChat,
        auth: AuthKind::Bearer,
        static_models: &[],
    },
    ProviderDescriptor {
        id: "synthetic",
        display_name: "Synthetic",
        base_url: "https://api.synthetic.new",
        models_path: "/v1/models",
        chat_path: "/v1/chat/completions",
        wire: WireProtocol::OpenAiChat,
        auth: AuthKind::Bearer,
        static_models: &[],
    },
];

pub fn providers() -> &'static [ProviderDescriptor] {
    PROVIDERS
}

pub fn provider(id: &str) -> Option<&'static ProviderDescriptor> {
    PROVIDERS.iter().find(|provider| provider.id == id)
}

pub fn route(value: &str) -> Option<(&'static ProviderDescriptor, &str)> {
    let (provider_id, model_id) = value.split_once('/')?;
    let descriptor = provider(provider_id)?;
    valid_model_id(model_id).then_some((descriptor, model_id))
}

fn valid_model_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 512
        && value.trim() == value
        && !value.chars().any(char::is_control)
}

fn credential_key(secret: &str) -> Result<String, String> {
    let trimmed = secret.trim();
    if trimmed.is_empty() {
        return Err("Skarbiec returned an empty provider credential".into());
    }
    let Ok(value) = serde_json::from_str::<Value>(trimmed) else {
        return Ok(trimmed.to_string());
    };
    let candidates = [
        value.pointer("/key"),
        value.pointer("/apiKey"),
        value.pointer("/api_key"),
        value.pointer("/access"),
        value.pointer("/accessToken"),
        value.pointer("/access_token"),
        value.pointer("/token"),
        value.pointer("/tokens/access_token"),
        value.pointer("/claudeAiOauth/accessToken"),
    ];
    let key = candidates
        .into_iter()
        .flatten()
        .find_map(Value::as_str)
        .map(str::to_string)
        .filter(|value| !value.trim().is_empty());
    key.ok_or_else(|| "Skarbiec provider credential has no supported key field".to_string())
}

fn endpoint(base_url: &str, path: &str) -> String {
    format!("{}{}", base_url.trim_end_matches('/'), path)
}

fn provider_base_url(descriptor: &ProviderDescriptor) -> String {
    let suffix = descriptor
        .id
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect::<String>();
    std::env::var(format!("BRAMA_PROVIDER_{suffix}_BASE_URL"))
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| descriptor.base_url.to_string())
}

fn authorize(
    builder: RequestBuilder,
    descriptor: &ProviderDescriptor,
    key: &str,
) -> RequestBuilder {
    match descriptor.auth {
        AuthKind::Bearer => builder.bearer_auth(key),
        AuthKind::XApiKey => builder
            .header("x-api-key", key)
            .header("anthropic-version", "2023-06-01"),
    }
}

fn model_from_value(descriptor: &ProviderDescriptor, row: &Value) -> Option<RegistryModel> {
    let id = row
        .get("id")
        .or_else(|| row.get("name"))
        .and_then(Value::as_str)?;
    if !valid_model_id(id) {
        return None;
    }
    let context_window = ["context_window", "context_length", "max_model_len"]
        .into_iter()
        .find_map(|key| row.get(key).and_then(Value::as_u64))
        .unwrap_or(128_000);
    let max_output_tokens = ["max_output_tokens", "max_tokens"]
        .into_iter()
        .find_map(|key| row.get(key).and_then(Value::as_u64))
        .unwrap_or(16_384);
    let lower = id.to_ascii_lowercase();
    Some(RegistryModel {
        route_id: format!("{}/{}", descriptor.id, id),
        provider_id: descriptor.id.to_string(),
        model_id: id.to_string(),
        context_window,
        max_output_tokens,
        input_modalities: vec!["text".into()],
        tools: true,
        reasoning: lower.contains("reason")
            || lower.contains("thinking")
            || lower.contains("deepseek-r1")
            || lower.contains("o1")
            || lower.contains("o3")
            || lower.contains("o4"),
    })
}

pub async fn discover_models(
    provider_id: &str,
    secret: &str,
) -> Result<Vec<RegistryModel>, String> {
    let descriptor = provider(provider_id)
        .ok_or_else(|| format!("provider `{provider_id}` is not in the Wisent registry"))?;
    let key = credential_key(secret)?;
    let client = Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .map_err(|error| error.to_string())?;
    let request = authorize(
        client.get(endpoint(
            &provider_base_url(descriptor),
            descriptor.models_path,
        )),
        descriptor,
        &key,
    );
    let dynamic = match request.send().await {
        Ok(response) if response.status().is_success() => response
            .json::<Value>()
            .await
            .ok()
            .and_then(|body| {
                body.get("data")
                    .or_else(|| body.get("models"))
                    .and_then(Value::as_array)
                    .cloned()
            })
            .unwrap_or_default(),
        _ => Vec::new(),
    };
    let mut models = dynamic
        .iter()
        .filter_map(|row| model_from_value(descriptor, row))
        .collect::<Vec<_>>();
    models.extend(
        descriptor
            .static_models
            .iter()
            .filter_map(|id| model_from_value(descriptor, &json!({"id": id}))),
    );
    models.sort_by(|left, right| left.route_id.cmp(&right.route_id));
    models.dedup_by(|left, right| left.route_id == right.route_id);
    if models.is_empty() {
        return Err(format!(
            "provider `{provider_id}` returned no models and has no static fallback"
        ));
    }
    Ok(models)
}

fn openai_messages(request: &ModelRequest) -> Vec<Value> {
    let mut messages =
        Vec::with_capacity(request.messages.len() + usize::from(request.system.is_some()));
    if let Some(system) = request.system.as_deref().filter(|value| !value.is_empty()) {
        messages.push(json!({"role": "system", "content": system}));
    }
    messages.extend(request.messages.iter().map(|message| {
        let mut value = serde_json::to_value(message).unwrap_or_else(|_| {
            json!({
                "role": message.role,
                "content": message.content,
            })
        });
        if let Some(object) = value.as_object_mut() {
            object.remove("name");
        }
        value
    }));
    messages
}

fn anthropic_content(message: &Message) -> Value {
    if message.role == "tool" {
        return json!([{
            "type": "tool_result",
            "tool_use_id": message.tool_call_id,
            "content": message.content_text(),
        }]);
    }
    if let Some(calls) = &message.tool_calls {
        let mut blocks = Vec::new();
        let text = message.content_text();
        if !text.is_empty() {
            blocks.push(json!({"type": "text", "text": text}));
        }
        for call in calls {
            let Some(function) = call.get("function") else {
                continue;
            };
            let input = function
                .get("arguments")
                .and_then(Value::as_str)
                .and_then(|value| serde_json::from_str::<Value>(value).ok())
                .unwrap_or_else(|| json!({}));
            blocks.push(json!({
                "type": "tool_use",
                "id": call.get("id").and_then(Value::as_str).unwrap_or("tool"),
                "name": function.get("name").and_then(Value::as_str).unwrap_or("tool"),
                "input": input,
            }));
        }
        return Value::Array(blocks);
    }
    message.content.clone()
}

fn anthropic_messages(request: &ModelRequest) -> Vec<Value> {
    request
        .messages
        .iter()
        .map(|message| {
            let role = if message.role == "assistant" {
                "assistant"
            } else {
                "user"
            };
            json!({"role": role, "content": anthropic_content(message)})
        })
        .collect()
}

fn anthropic_tools(request: &ModelRequest) -> Option<Vec<Value>> {
    request.tools.as_ref().map(|tools| {
        tools
            .iter()
            .map(|tool| json!({
                "name": tool.function.name,
                "description": tool.function.description,
                "input_schema": tool.function.parameters.clone().unwrap_or_else(|| json!({"type": "object"})),
            }))
            .collect()
    })
}

fn model_response_from_openai(route_id: &str, body: Value, elapsed_ms: f64) -> ModelResponse {
    let choice = body
        .pointer("/choices/0/message")
        .cloned()
        .unwrap_or(Value::Null);
    let content = choice
        .get("content")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let tool_calls = choice
        .get("tool_calls")
        .and_then(Value::as_array)
        .map(|calls| {
            calls
                .iter()
                .filter_map(|call| serde_json::from_value::<ToolCall>(call.clone()).ok())
                .collect::<Vec<_>>()
        })
        .filter(|calls| !calls.is_empty());
    ModelResponse {
        content,
        model: route_id.to_string(),
        input_tokens: body
            .pointer("/usage/prompt_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0) as u32,
        output_tokens: body
            .pointer("/usage/completion_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0) as u32,
        latency_ms: elapsed_ms,
        cost: 0.0,
        success: true,
        error: None,
        tool_calls,
    }
}

fn model_response_from_anthropic(route_id: &str, body: Value, elapsed_ms: f64) -> ModelResponse {
    let mut text = String::new();
    let mut tool_calls = Vec::new();
    for block in body
        .get("content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        match block.get("type").and_then(Value::as_str) {
            Some("text") => text.push_str(
                block
                    .get("text")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
            ),
            Some("tool_use") => tool_calls.push(ToolCall {
                id: block
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or("tool")
                    .to_string(),
                call_type: "function".into(),
                function: crate::types::ToolCallFunction {
                    name: block
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or("tool")
                        .to_string(),
                    arguments: block
                        .get("input")
                        .cloned()
                        .unwrap_or_else(|| json!({}))
                        .to_string(),
                },
            }),
            _ => {}
        }
    }
    ModelResponse {
        content: text,
        model: route_id.to_string(),
        input_tokens: body
            .pointer("/usage/input_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0) as u32,
        output_tokens: body
            .pointer("/usage/output_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0) as u32,
        latency_ms: elapsed_ms,
        cost: 0.0,
        success: true,
        error: None,
        tool_calls: (!tool_calls.is_empty()).then_some(tool_calls),
    }
}

fn provider_error(route_id: &str, status: reqwest::StatusCode, body: &str) -> ModelResponse {
    let message = serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|value| {
            value
                .pointer("/error/message")
                .or_else(|| value.get("message"))
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_else(|| format!("provider returned HTTP {}", status.as_u16()));
    ModelResponse::failure(route_id, message)
}

pub async fn dispatch(request: &ModelRequest, secret: &str) -> ModelResponse {
    let Some((descriptor, model_id)) = route(&request.model) else {
        return ModelResponse::failure(&request.model, "unknown Wisent provider route".into());
    };
    let key = match credential_key(secret) {
        Ok(key) => key,
        Err(error) => return ModelResponse::failure(&request.model, error),
    };
    let client = match Client::builder()
        .timeout(std::time::Duration::from_secs(255))
        .build()
    {
        Ok(client) => client,
        Err(error) => return ModelResponse::failure(&request.model, error.to_string()),
    };
    let payload = match descriptor.wire {
        WireProtocol::OpenAiChat => {
            let mut body = Map::new();
            body.insert("model".into(), json!(model_id));
            body.insert("messages".into(), Value::Array(openai_messages(request)));
            body.insert("max_tokens".into(), json!(request.max_tokens));
            body.insert("temperature".into(), json!(request.temperature));
            if let Some(tools) = &request.tools {
                body.insert(
                    "tools".into(),
                    serde_json::to_value(tools).unwrap_or(Value::Null),
                );
            }
            Value::Object(body)
        }
        WireProtocol::AnthropicMessages => {
            let mut body = json!({
                "model": model_id,
                "messages": anthropic_messages(request),
                "max_tokens": request.max_tokens,
                "temperature": request.temperature,
            });
            if let Some(system) = request.system.as_deref().filter(|value| !value.is_empty()) {
                body["system"] = json!(system);
            }
            if let Some(tools) = anthropic_tools(request) {
                body["tools"] = json!(tools);
            }
            body
        }
    };
    let started = Instant::now();
    let response = match authorize(
        client.post(endpoint(
            &provider_base_url(descriptor),
            descriptor.chat_path,
        )),
        descriptor,
        &key,
    )
    .json(&payload)
    .send()
    .await
    {
        Ok(response) => response,
        Err(error) => return ModelResponse::failure(&request.model, error.to_string()),
    };
    let status = response.status();
    let text = match response.text().await {
        Ok(text) => text,
        Err(error) => return ModelResponse::failure(&request.model, error.to_string()),
    };
    if !status.is_success() {
        return provider_error(&request.model, status, &text);
    }
    let body = match serde_json::from_str::<Value>(&text) {
        Ok(body) => body,
        Err(error) => {
            return ModelResponse::failure(
                &request.model,
                format!("invalid provider response: {error}"),
            )
        }
    };
    let elapsed_ms = started.elapsed().as_secs_f64() * 1_000.0;
    match descriptor.wire {
        WireProtocol::OpenAiChat => model_response_from_openai(&request.model, body, elapsed_ms),
        WireProtocol::AnthropicMessages => {
            model_response_from_anthropic(&request.model, body, elapsed_ms)
        }
    }
}
