//! Native Wisent provider registry for credentials redeemed from Skarbiec.
//!
//! This module is intentionally independent from external agent harnesses. It owns
//! provider discovery, request shaping and response normalization for API-backed
//! subscriptions. Secrets are passed in-memory by the Skarbiec capability broker
//! and are never persisted here.

use std::collections::HashMap;
use std::sync::LazyLock;
use std::time::Instant;

use reqwest::{Client, RequestBuilder};
use serde_json::{json, Map, Value};

use super::model_catalog::{self, CatalogAuth, CatalogProtocol, CatalogProvider};
use crate::types::{Message, ModelRequest, ModelResponse, ToolCall};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WireProtocol {
    OpenAiChat,
    AnthropicMessages,
    OpenAiResponses,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthKind {
    Bearer,
    XApiKey,
    AnthropicBearer,
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
    pub input_price: f64,
    pub output_price: f64,
    pub cache_read_price: f64,
    pub cache_write_price: f64,
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
        id: "claude-code",
        display_name: "Claude Code (subscription)",
        base_url: "https://api.anthropic.com",
        models_path: "/v1/models",
        chat_path: "/v1/messages",
        wire: WireProtocol::AnthropicMessages,
        auth: AuthKind::AnthropicBearer,
        static_models: &["claude-haiku-4-5", "claude-opus-4-6", "claude-sonnet-4-6"],
    },
    ProviderDescriptor {
        id: "kimi",
        display_name: "Kimi (subscription)",
        base_url: "https://api.kimi.com/coding",
        models_path: "/v1/models",
        chat_path: "/v1/chat/completions",
        wire: WireProtocol::OpenAiChat,
        auth: AuthKind::Bearer,
        static_models: &["kimi-for-coding"],
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
        id: "codex",
        display_name: "Codex (ChatGPT subscription)",
        base_url: "https://chatgpt.com/backend-api/codex",
        models_path: "/models",
        chat_path: "/responses",
        wire: WireProtocol::OpenAiResponses,
        auth: AuthKind::Bearer,
        static_models: &[
            "gpt-5.6-sol",
            "gpt-5.6-luna",
            "gpt-5.6-terra",
            "gpt-5.5",
            "gpt-5.3-codex-spark",
        ],
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
pub fn provider_id_from_route(value: &str) -> Option<&str> {
    let (provider_id, model_id) = value.split_once('/')?;
    (valid_provider_id(provider_id) && valid_model_id(model_id)).then_some(provider_id)
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

fn valid_provider_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

fn credential_base_url(secret: &str) -> Option<String> {
    let value = serde_json::from_str::<Value>(secret.trim()).ok()?;
    let result = [
        value.pointer("/base_url"),
        value.pointer("/baseUrl"),
        value.pointer("/api_base"),
        value.pointer("/apiBase"),
        value.pointer("/endpoint"),
    ]
    .into_iter()
    .flatten()
    .find_map(Value::as_str)
    .filter(|value| value.starts_with("https://") || value.starts_with("http://"))
    .map(str::to_string);
    result
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

fn credential_account_id(secret: &str) -> Option<String> {
    let value = serde_json::from_str::<Value>(secret.trim()).ok()?;
    value
        .pointer("/tokens/account_id")
        .and_then(Value::as_str)
        .map(str::to_string)
        .filter(|value| !value.trim().is_empty())
}

fn endpoint(base_url: &str, path: &str) -> String {
    format!("{}{}", base_url.trim_end_matches('/'), path)
}
fn catalog_endpoint(base_url: &str, path: &str) -> String {
    let base = base_url.trim_end_matches('/');
    if base.ends_with(path) {
        return base.to_string();
    }
    if path == "/models" {
        if let Some(prefix) = base
            .strip_suffix("/chat/completions")
            .or_else(|| base.strip_suffix("/messages"))
        {
            return endpoint(prefix, path);
        }
    }
    endpoint(base, path)
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
        AuthKind::AnthropicBearer => builder
            .bearer_auth(key)
            .header("anthropic-version", "2023-06-01")
            .header("anthropic-beta", "oauth-2025-04-20"),
    }
}

/// `authorize` plus the extra headers the Codex ChatGPT-account backend
/// requires. A no-op for every other provider, so their requests stay
/// byte-identical.
fn authorize_provider(
    builder: RequestBuilder,
    descriptor: &ProviderDescriptor,
    key: &str,
    secret: &str,
) -> RequestBuilder {
    let builder = authorize(builder, descriptor, key);
    if descriptor.wire != WireProtocol::OpenAiResponses {
        return builder;
    }
    let builder = builder
        .header("OpenAI-Beta", "responses=experimental")
        .header("originator", "codex_cli_rs");
    match credential_account_id(secret) {
        Some(account_id) => builder.header("chatgpt-account-id", account_id),
        None => builder,
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
        input_price: 0.0,
        output_price: 0.0,
        cache_read_price: 0.0,
        cache_write_price: 0.0,
    })
}

/// oh-my-pi model metadata extracted from the local models.db, embedded at
/// compile time so subscription providers get authoritative limits.
static OMP_MODEL_METADATA: &str = include_str!("../../scripts/omp-model-metadata.json");

#[derive(Clone, Debug)]
struct OmpModelMetadata {
    context_window: u64,
    max_output_tokens: u64,
    reasoning: bool,
    input_modalities: Vec<String>,
}

/// Parsed view of `OMP_MODEL_METADATA`: provider_id -> model_id -> metadata.
/// Malformed entries are skipped instead of failing the whole table.
static OMP_METADATA: LazyLock<HashMap<String, HashMap<String, OmpModelMetadata>>> =
    LazyLock::new(|| {
        let mut providers = HashMap::new();
        let Ok(Value::Object(entries)) = serde_json::from_str::<Value>(OMP_MODEL_METADATA) else {
            return providers;
        };
        for (provider_id, models) in entries {
            let Some(rows) = models.as_array() else {
                continue;
            };
            let mut table = HashMap::new();
            for row in rows {
                if let Some((model_id, metadata)) = omp_metadata_from_value(row) {
                    table.insert(model_id, metadata);
                }
            }
            providers.insert(provider_id, table);
        }
        providers
    });

fn omp_metadata_from_value(row: &Value) -> Option<(String, OmpModelMetadata)> {
    let model_id = row.get("id")?.as_str()?;
    let context_window = row.get("contextWindow")?.as_u64()?;
    let max_output_tokens = row.get("maxTokens")?.as_u64()?;
    let reasoning = row.get("reasoning")?.as_bool()?;
    let input_modalities = row
        .get("input")?
        .as_array()?
        .iter()
        .filter_map(|value| value.as_str().map(str::to_string))
        .collect::<Vec<_>>();
    if input_modalities.is_empty() {
        return None;
    }
    Some((
        model_id.to_string(),
        OmpModelMetadata {
            context_window,
            max_output_tokens,
            reasoning,
            input_modalities,
        },
    ))
}

/// Override discovered models with authoritative oh-my-pi metadata on a
/// `model_id` match. Only providers present in the embedded table
/// ("claude-code", "codex", "kimi") are affected; route/model ids, tools and
/// prices are left untouched.
fn apply_omp_model_metadata(provider_id: &str, models: &mut [RegistryModel]) {
    let Some(table) = OMP_METADATA.get(provider_id) else {
        return;
    };
    for model in models.iter_mut() {
        if let Some(metadata) = table.get(&model.model_id) {
            model.context_window = metadata.context_window;
            model.max_output_tokens = metadata.max_output_tokens;
            model.reasoning = metadata.reasoning;
            model.input_modalities = metadata.input_modalities.clone();
        }
    }
}

fn catalog_provider_base_url(descriptor: &CatalogProvider, secret: &str) -> Option<String> {
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
        .or_else(|| credential_base_url(secret))
        .or_else(|| descriptor.api_base.clone())
}

fn authorize_catalog(
    builder: RequestBuilder,
    descriptor: &CatalogProvider,
    key: &str,
) -> RequestBuilder {
    match descriptor.auth {
        CatalogAuth::Bearer => builder.bearer_auth(key),
        CatalogAuth::XApiKey => builder
            .header("x-api-key", key)
            .header("anthropic-version", "2023-06-01"),
        CatalogAuth::GoogleApiKey => builder.header("x-goog-api-key", key),
    }
}

fn catalog_model_from_value(provider_id: &str, row: &Value) -> Option<RegistryModel> {
    let id = row
        .get("id")
        .or_else(|| row.get("name"))
        .and_then(Value::as_str)?
        .strip_prefix("models/")
        .unwrap_or_else(|| {
            row.get("id")
                .or_else(|| row.get("name"))
                .and_then(Value::as_str)
                .unwrap_or_default()
        });
    if !valid_model_id(id) {
        return None;
    }
    Some(RegistryModel {
        route_id: format!("{provider_id}/{id}"),
        provider_id: provider_id.to_string(),
        model_id: id.to_string(),
        context_window: row
            .get("inputTokenLimit")
            .or_else(|| row.get("context_window"))
            .or_else(|| row.get("context_length"))
            .and_then(Value::as_u64)
            .unwrap_or(128_000),
        max_output_tokens: row
            .get("outputTokenLimit")
            .or_else(|| row.get("max_output_tokens"))
            .or_else(|| row.get("max_tokens"))
            .and_then(Value::as_u64)
            .unwrap_or(16_384),
        input_modalities: vec!["text".into()],
        tools: true,
        reasoning: false,
        input_price: 0.0,
        output_price: 0.0,
        cache_read_price: 0.0,
        cache_write_price: 0.0,
    })
}

pub async fn discover_models(
    provider_id: &str,
    secret: &str,
) -> Result<Vec<RegistryModel>, String> {
    let catalog = model_catalog::snapshot().await?;
    if let Some(descriptor) = catalog.providers.get(provider_id) {
        if !descriptor.executable() {
            return Err(format!(
                "provider `{provider_id}` uses a protocol not implemented by Brama"
            ));
        }
        let mut models = catalog
            .models
            .iter()
            .filter(|model| model.provider_id == provider_id)
            .cloned()
            .collect::<Vec<_>>();
        let key = credential_key(secret)?;
        if let Some(base_url) = catalog_provider_base_url(descriptor, secret) {
            let client = Client::builder()
                .timeout(std::time::Duration::from_secs(20))
                .build()
                .map_err(|error| error.to_string())?;
            let request = authorize_catalog(
                client.get(catalog_endpoint(&base_url, "/models")),
                descriptor,
                &key,
            );
            if let Ok(response) = request.send().await {
                if response.status().is_success() {
                    if let Ok(body) = response.json::<Value>().await {
                        let dynamic = body
                            .get("data")
                            .or_else(|| body.get("models"))
                            .and_then(Value::as_array)
                            .cloned()
                            .unwrap_or_default();
                        models.extend(
                            dynamic
                                .iter()
                                .filter_map(|row| catalog_model_from_value(provider_id, row)),
                        );
                    }
                }
            }
        }
        models.sort_by(|left, right| left.route_id.cmp(&right.route_id));
        models.dedup_by(|left, right| left.route_id == right.route_id);
        if models.is_empty() {
            return Err(format!("provider `{provider_id}` has no catalog models"));
        }
        return Ok(models);
    }

    let descriptor = provider(provider_id)
        .ok_or_else(|| format!("provider `{provider_id}` is not in the Wisent registry"))?;
    let key = credential_key(secret)?;
    let client = Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .map_err(|error| error.to_string())?;
    let request = authorize_provider(
        client.get(endpoint(
            &provider_base_url(descriptor),
            descriptor.models_path,
        )),
        descriptor,
        &key,
        secret,
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
    apply_omp_model_metadata(provider_id, &mut models);
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

/// Some providers (e.g. Moonshot/Kimi) reject JSON Schemas whose nodes lack an
/// explicit `type`. Recursively infer a conservative `type` where it is
/// missing: object when `properties` is present, array when `items` is
/// present, the enum's first value kind, otherwise string.
/// Keys whose presence marks an object as a JSON Schema node (as opposed to a
/// plain map like the `properties` object itself).
const SCHEMA_HINT_KEYS: &[&str] = &[
    "enum",
    "const",
    "items",
    "properties",
    "required",
    "additionalProperties",
    "anyOf",
    "oneOf",
    "allOf",
    "not",
    "patternProperties",
    "description",
    "format",
    "pattern",
    "default",
    "examples",
    "minimum",
    "maximum",
    "minLength",
    "maxLength",
    "minItems",
    "maxItems",
    "uniqueItems",
    "multipleOf",
];

fn is_schema_node(map: &serde_json::Map<String, Value>) -> bool {
    SCHEMA_HINT_KEYS.iter().any(|key| map.contains_key(*key))
}

fn normalize_json_schema(value: &mut Value) {
    if let Some(map) = value.as_object_mut() {
        // jeden's action schemas allow shorthand properties like
        // {"properties": {"type": "string"}} — a bare type name instead of a
        // schema object. Unwrap those before providers see them.
        if let Some(properties) = map.get_mut("properties").and_then(Value::as_object_mut) {
            for property in properties.values_mut() {
                if let Some(name) = property.as_str() {
                    let valid = matches!(
                        name,
                        "string" | "number" | "integer" | "boolean" | "object" | "array" | "null"
                    );
                    *property = json!({ "type": if valid { name } else { "string" } });
                }
                normalize_json_schema(property);
            }
        }
        if !map.contains_key("type") && is_schema_node(map) {
            let inferred = if map.contains_key("properties") {
                "object"
            } else if map.contains_key("items") {
                "array"
            } else {
                match map
                    .get("enum")
                    .and_then(Value::as_array)
                    .and_then(|values| values.first())
                {
                    Some(Value::Number(_)) => "number",
                    Some(Value::Bool(_)) => "boolean",
                    _ => "string",
                }
            };
            map.insert("type".into(), Value::String(inferred.into()));
        }
        for key in [
            "items",
            "additionalProperties",
            "anyOf",
            "oneOf",
            "allOf",
            "not",
            "patternProperties",
            "$defs",
            "definitions",
        ] {
            if let Some(child) = map.get_mut(key) {
                normalize_json_schema(child);
            }
        }
    }
    if let Some(items) = value.as_array_mut() {
        for item in items {
            normalize_json_schema(item);
        }
    }
}

fn normalized_tools_value<T: serde::Serialize>(tools: &T) -> Value {
    let mut value = serde_json::to_value(tools).unwrap_or(Value::Null);
    if let Some(array) = value.as_array_mut() {
        for tool in array {
            if let Some(parameters) = tool.pointer_mut("/function/parameters") {
                normalize_json_schema(parameters);
            }
        }
    }
    value
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

fn responses_input(request: &ModelRequest) -> Vec<Value> {
    let mut input = Vec::with_capacity(request.messages.len());
    for message in &request.messages {
        if message.role == "tool" {
            input.push(json!({
                "type": "function_call_output",
                "call_id": message.tool_call_id.as_deref().unwrap_or("tool"),
                "output": message.content_text(),
            }));
            continue;
        }
        if let Some(calls) = &message.tool_calls {
            let text = message.content_text();
            if !text.is_empty() {
                input.push(json!({
                    "type": "message",
                    "role": "assistant",
                    "content": [{"type": "output_text", "text": text}],
                }));
            }
            for call in calls {
                let Some(function) = call.get("function") else {
                    continue;
                };
                input.push(json!({
                    "type": "function_call",
                    "call_id": call.get("id").and_then(Value::as_str).unwrap_or("tool"),
                    "name": function.get("name").and_then(Value::as_str).unwrap_or("tool"),
                    "arguments": function
                        .get("arguments")
                        .and_then(Value::as_str)
                        .unwrap_or("{}"),
                }));
            }
            continue;
        }
        let role = if message.role == "assistant" {
            "assistant"
        } else {
            "user"
        };
        let content_type = if role == "assistant" {
            "output_text"
        } else {
            "input_text"
        };
        input.push(json!({
            "type": "message",
            "role": role,
            "content": [{"type": content_type, "text": message.content_text()}],
        }));
    }
    input
}

fn responses_tools(request: &ModelRequest) -> Option<Vec<Value>> {
    request
        .tools
        .as_ref()
        .filter(|tools| !tools.is_empty())
        .map(|tools| {
            tools
                .iter()
                .map(|tool| json!({
                    "type": "function",
                    "name": tool.function.name,
                    "description": tool.function.description,
                    "parameters": tool.function.parameters.clone().unwrap_or_else(|| json!({"type": "object"})),
                }))
                .collect()
        })
}

fn responses_payload(request: &ModelRequest, model_id: &str) -> Value {
    let mut body = json!({
        "model": model_id,
        "input": responses_input(request),
        "store": false,
        "stream": true,
    });
    if let Some(system) = request.system.as_deref().filter(|value| !value.is_empty()) {
        body["instructions"] = json!(system);
    }
    if let Some(tools) = responses_tools(request) {
        body["tools"] = json!(tools);
        body["tool_choice"] = json!("auto");
    }
    body
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

/// Parse a buffered `text/event-stream` body from the OpenAI Responses API
/// into the shared response shape. Deltas accumulate content, but a
/// `response.completed` event's `output` array is the final source of truth.
fn model_response_from_responses_stream(
    route_id: &str,
    body: &str,
    elapsed_ms: f64,
) -> ModelResponse {
    let mut content = String::new();
    let mut completed = Value::Null;
    let mut failure = None;
    for line in body.lines() {
        let Some(data) = line.strip_prefix("data:") else {
            continue;
        };
        let data = data.trim();
        if data.is_empty() || data == "[DONE]" {
            continue;
        }
        let Ok(event) = serde_json::from_str::<Value>(data) else {
            continue;
        };
        match event.get("type").and_then(Value::as_str) {
            Some("response.output_text.delta") => {
                if let Some(delta) = event.get("delta").and_then(Value::as_str) {
                    content.push_str(delta);
                }
            }
            Some("response.completed") => {
                completed = event.get("response").cloned().unwrap_or(Value::Null);
            }
            Some("response.failed") => {
                let message = event
                    .pointer("/response/error/message")
                    .and_then(Value::as_str)
                    .unwrap_or("codex response failed");
                failure = Some(message.to_string());
            }
            Some("error") => {
                let message = event
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("codex stream error");
                failure = Some(message.to_string());
            }
            _ => {}
        }
    }
    if let Some(message) = failure {
        return ModelResponse::failure(route_id, message);
    }
    let mut tool_calls = Vec::new();
    if let Some(output) = completed.get("output").and_then(Value::as_array) {
        let mut text = String::new();
        for item in output {
            match item.get("type").and_then(Value::as_str) {
                Some("message") => {
                    for part in item
                        .get("content")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                    {
                        if let Some(value) = part.get("text").and_then(Value::as_str) {
                            text.push_str(value);
                        }
                    }
                }
                Some("function_call") => tool_calls.push(ToolCall {
                    id: item
                        .get("call_id")
                        .or_else(|| item.get("id"))
                        .and_then(Value::as_str)
                        .unwrap_or("tool")
                        .to_string(),
                    call_type: "function".into(),
                    function: crate::types::ToolCallFunction {
                        name: item
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or("tool")
                            .to_string(),
                        arguments: item
                            .get("arguments")
                            .and_then(Value::as_str)
                            .unwrap_or("{}")
                            .to_string(),
                    },
                }),
                _ => {}
            }
        }
        if !text.is_empty() {
            content = text;
        }
    }
    let usage = completed.get("usage");
    ModelResponse {
        content,
        model: route_id.to_string(),
        input_tokens: usage
            .and_then(|usage| usage.get("input_tokens"))
            .and_then(Value::as_u64)
            .unwrap_or(0) as u32,
        output_tokens: usage
            .and_then(|usage| usage.get("output_tokens"))
            .and_then(Value::as_u64)
            .unwrap_or(0) as u32,
        latency_ms: elapsed_ms,
        cost: 0.0,
        success: true,
        error: None,
        tool_calls: (!tool_calls.is_empty()).then_some(tool_calls),
    }
}

fn google_parts(message: &Message) -> Vec<Value> {
    if message.role == "tool" {
        return vec![json!({
            "functionResponse": {
                "name": message.name.as_deref().unwrap_or("tool"),
                "response": {"result": message.content_text()},
            }
        })];
    }
    let mut parts = match &message.content {
        Value::String(text) => vec![json!({"text": text})],
        Value::Array(values) => values
            .iter()
            .filter_map(|part| match part.get("type").and_then(Value::as_str) {
                Some("text") => part.get("text").map(|text| json!({"text": text})),
                Some("image_url") => {
                    let url = part.pointer("/image_url/url").and_then(Value::as_str)?;
                    if let Some(data) = url.strip_prefix("data:") {
                        let (mime, encoded) = data.split_once(";base64,")?;
                        Some(json!({"inlineData": {"mimeType": mime, "data": encoded}}))
                    } else {
                        Some(json!({"fileData": {"fileUri": url}}))
                    }
                }
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    };
    if let Some(calls) = &message.tool_calls {
        parts.extend(calls.iter().filter_map(|call| {
            let function = call.get("function")?;
            let arguments = function
                .get("arguments")
                .and_then(Value::as_str)
                .and_then(|value| serde_json::from_str::<Value>(value).ok())
                .unwrap_or_else(|| json!({}));
            Some(json!({
                "functionCall": {
                    "name": function.get("name").and_then(Value::as_str).unwrap_or("tool"),
                    "args": arguments,
                }
            }))
        }));
    }
    parts
}

fn google_payload(request: &ModelRequest) -> Value {
    let mut body = json!({
        "contents": request.messages.iter().map(|message| {
            json!({
                "role": if message.role == "assistant" { "model" } else { "user" },
                "parts": google_parts(message),
            })
        }).collect::<Vec<_>>(),
        "generationConfig": {
            "maxOutputTokens": request.max_tokens,
            "temperature": request.temperature,
        },
    });
    if let Some(system) = request.system.as_deref().filter(|value| !value.is_empty()) {
        body["systemInstruction"] = json!({"parts": [{"text": system}]});
    }
    if let Some(tools) = request.tools.as_ref().filter(|tools| !tools.is_empty()) {
        body["tools"] = json!([{
            "functionDeclarations": tools.iter().map(|tool| json!({
                "name": tool.function.name,
                "description": tool.function.description,
                "parameters": tool.function.parameters.clone().unwrap_or_else(|| json!({"type": "object"})),
            })).collect::<Vec<_>>()
        }]);
    }
    body
}

fn model_response_from_google(route_id: &str, body: Value, elapsed_ms: f64) -> ModelResponse {
    let mut content = String::new();
    let mut tool_calls = Vec::new();
    for (index, part) in body
        .pointer("/candidates/0/content/parts")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .enumerate()
    {
        if let Some(text) = part.get("text").and_then(Value::as_str) {
            content.push_str(text);
        }
        if let Some(call) = part.get("functionCall") {
            tool_calls.push(ToolCall {
                id: format!("google-call-{index}"),
                call_type: "function".into(),
                function: crate::types::ToolCallFunction {
                    name: call
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or("tool")
                        .to_string(),
                    arguments: call
                        .get("args")
                        .cloned()
                        .unwrap_or_else(|| json!({}))
                        .to_string(),
                },
            });
        }
    }
    ModelResponse {
        content,
        model: route_id.to_string(),
        input_tokens: body
            .pointer("/usageMetadata/promptTokenCount")
            .and_then(Value::as_u64)
            .unwrap_or(0) as u32,
        output_tokens: body
            .pointer("/usageMetadata/candidatesTokenCount")
            .and_then(Value::as_u64)
            .unwrap_or(0) as u32,
        latency_ms: elapsed_ms,
        cost: 0.0,
        success: true,
        error: None,
        tool_calls: (!tool_calls.is_empty()).then_some(tool_calls),
    }
}

async fn dispatch_catalog(request: &ModelRequest, secret: &str) -> ModelResponse {
    let Some((provider_id, model_id)) = request.model.split_once('/') else {
        return ModelResponse::failure(&request.model, "invalid provider/model route".into());
    };
    if !valid_provider_id(provider_id) || !valid_model_id(model_id) {
        return ModelResponse::failure(&request.model, "invalid provider/model route".into());
    }
    let catalog = match model_catalog::snapshot().await {
        Ok(catalog) => catalog,
        Err(error) => return ModelResponse::failure(&request.model, error),
    };
    let Some(descriptor) = catalog.providers.get(provider_id) else {
        return ModelResponse::failure(
            &request.model,
            format!("provider `{provider_id}` is not in the Wisent catalog"),
        );
    };
    if !descriptor.executable() {
        return ModelResponse::failure(
            &request.model,
            format!("provider `{provider_id}` uses an unsupported protocol"),
        );
    }
    if !catalog
        .models
        .iter()
        .any(|model| model.route_id == request.model)
    {
        return ModelResponse::failure(
            &request.model,
            format!(
                "model `{}` is not advertised by provider `{provider_id}`",
                model_id
            ),
        );
    }
    let key = match credential_key(secret) {
        Ok(key) => key,
        Err(error) => return ModelResponse::failure(&request.model, error),
    };
    let Some(base_url) = catalog_provider_base_url(descriptor, secret) else {
        return ModelResponse::failure(
            &request.model,
            format!("provider `{provider_id}` has no API endpoint"),
        );
    };
    let client = match Client::builder()
        .timeout(std::time::Duration::from_secs(255))
        .build()
    {
        Ok(client) => client,
        Err(error) => return ModelResponse::failure(&request.model, error.to_string()),
    };
    let (url, payload) = match descriptor.protocol {
        CatalogProtocol::OpenAiChat => {
            let mut body = Map::new();
            body.insert("model".into(), json!(model_id));
            body.insert("messages".into(), Value::Array(openai_messages(request)));
            body.insert("max_tokens".into(), json!(request.max_tokens));
            body.insert("temperature".into(), json!(request.temperature));
            if let Some(tools) = &request.tools {
                body.insert(
                    "tools".into(),
                    normalized_tools_value(tools),
                );
            }
            (
                catalog_endpoint(&base_url, "/chat/completions"),
                Value::Object(body),
            )
        }
        CatalogProtocol::AnthropicMessages => {
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
            (catalog_endpoint(&base_url, "/messages"), body)
        }
        CatalogProtocol::GoogleGenerateContent => {
            let mut url = match reqwest::Url::parse(&base_url) {
                Ok(url) => url,
                Err(error) => return ModelResponse::failure(&request.model, error.to_string()),
            };
            match url.path_segments_mut() {
                Ok(mut segments) => {
                    segments.pop_if_empty();
                    segments.push("models");
                    segments.push(&format!("{model_id}:generateContent"));
                }
                Err(()) => {
                    return ModelResponse::failure(
                        &request.model,
                        "provider API endpoint cannot be a base URL".into(),
                    )
                }
            }
            (url.to_string(), google_payload(request))
        }
        CatalogProtocol::Unsupported => unreachable!(),
    };
    let started = Instant::now();
    let response = match authorize_catalog(client.post(url), descriptor, &key)
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
    match descriptor.protocol {
        CatalogProtocol::OpenAiChat => model_response_from_openai(&request.model, body, elapsed_ms),
        CatalogProtocol::AnthropicMessages => {
            model_response_from_anthropic(&request.model, body, elapsed_ms)
        }
        CatalogProtocol::GoogleGenerateContent => {
            model_response_from_google(&request.model, body, elapsed_ms)
        }
        CatalogProtocol::Unsupported => unreachable!(),
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
        return dispatch_catalog(request, secret).await;
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
            // kimi-for-coding pins temperature to 1 and rejects any other value.
            if descriptor.id != "kimi" {
                body.insert("temperature".into(), json!(request.temperature));
            }
            if let Some(tools) = &request.tools {
                body.insert(
                    "tools".into(),
                    normalized_tools_value(tools),
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
        WireProtocol::OpenAiResponses => responses_payload(request, model_id),
    };
    let started = Instant::now();
    let response = match authorize_provider(
        client.post(endpoint(
            &provider_base_url(descriptor),
            descriptor.chat_path,
        )),
        descriptor,
        &key,
        secret,
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
    let elapsed_ms = started.elapsed().as_secs_f64() * 1_000.0;
    if descriptor.wire == WireProtocol::OpenAiResponses {
        return model_response_from_responses_stream(&request.model, &text, elapsed_ms);
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
    match descriptor.wire {
        WireProtocol::OpenAiChat => model_response_from_openai(&request.model, body, elapsed_ms),
        WireProtocol::AnthropicMessages => {
            model_response_from_anthropic(&request.model, body, elapsed_ms)
        }
        WireProtocol::OpenAiResponses => unreachable!(),
    }
}
