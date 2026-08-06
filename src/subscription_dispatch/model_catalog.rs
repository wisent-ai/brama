//! Independent Wisent model catalog.
//!
//! models.dev supplies public provider/model metadata only. Skarbiec remains the
//! credential authority and Weles remains the account/subscription authority.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};

use serde_json::Value;
use tokio::sync::{Mutex, RwLock};

use crate::providers::adapter::RegistryModel;

const DEFAULT_CATALOG_URL: &str = "https://models.dev/api.json";
const DEFAULT_CACHE_PATH: &str = "/tmp/brama-models-dev-cache.json";
const DEFAULT_TTL_SECONDS: u64 = 900;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CatalogProtocol {
    OpenAiChat,
    AnthropicMessages,
    GoogleGenerateContent,
    Unsupported,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CatalogAuth {
    Bearer,
    XApiKey,
    GoogleApiKey,
}

#[derive(Clone, Debug)]
pub struct CatalogProvider {
    pub id: String,
    pub display_name: String,
    pub protocol: CatalogProtocol,
    pub auth: CatalogAuth,
}

impl CatalogProvider {
    pub fn executable(&self) -> bool {
        self.protocol != CatalogProtocol::Unsupported
    }
}

#[derive(Clone, Debug)]
pub struct CatalogSnapshot {
    pub providers: HashMap<String, CatalogProvider>,
    pub models: Vec<RegistryModel>,
    pub revision: String,
}

struct CachedSnapshot {
    loaded_at: Instant,
    snapshot: Arc<CatalogSnapshot>,
}

static MEMORY_CACHE: LazyLock<RwLock<Option<CachedSnapshot>>> = LazyLock::new(|| RwLock::new(None));
static LOAD_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

pub async fn snapshot() -> Result<Arc<CatalogSnapshot>, String> {
    let ttl = Duration::from_secs(
        std::env::var("BRAMA_MODEL_CATALOG_TTL_SECONDS")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(DEFAULT_TTL_SECONDS),
    );
    {
        let cache = MEMORY_CACHE.read().await;
        if let Some(cached) = cache
            .as_ref()
            .filter(|cached| cached.loaded_at.elapsed() < ttl)
        {
            return Ok(Arc::clone(&cached.snapshot));
        }
    }
    let _load_guard = LOAD_LOCK.lock().await;
    {
        let cache = MEMORY_CACHE.read().await;
        if let Some(cached) = cache
            .as_ref()
            .filter(|cached| cached.loaded_at.elapsed() < ttl)
        {
            return Ok(Arc::clone(&cached.snapshot));
        }
    }

    let raw = match read_live_catalog().await {
        Ok(raw) => {
            if let Err(error) = write_cache(&raw).await {
                tracing::warn!(%error, "could not update models.dev cache");
            }
            raw
        }
        Err(live_error) => read_cache().await.map_err(|cache_error| {
            format!("models.dev unavailable ({live_error}); cache unavailable ({cache_error})")
        })?,
    };
    let parsed = Arc::new(parse_catalog(&raw)?);
    let mut cache = MEMORY_CACHE.write().await;
    if let Some(cached) = cache
        .as_ref()
        .filter(|cached| cached.loaded_at.elapsed() < ttl)
    {
        return Ok(Arc::clone(&cached.snapshot));
    }
    *cache = Some(CachedSnapshot {
        loaded_at: Instant::now(),
        snapshot: Arc::clone(&parsed),
    });
    Ok(parsed)
}

fn catalog_url() -> Result<reqwest::Url, String> {
    let raw = std::env::var("BRAMA_MODEL_CATALOG_URL")
        .unwrap_or_else(|_| DEFAULT_CATALOG_URL.to_string());
    let url = reqwest::Url::parse(raw.trim())
        .map_err(|error| format!("invalid model catalog URL: {error}"))?;
    if !url.username().is_empty() || url.password().is_some() {
        return Err("model catalog URL must not contain user info".into());
    }
    let host = url
        .host_str()
        .ok_or_else(|| "model catalog URL must contain a host".to_string())?;
    let loopback = host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|address| address.is_loopback());
    if url.scheme() != "https" && !(url.scheme() == "http" && loopback) {
        return Err("model catalog URL must use HTTPS or explicit loopback HTTP".into());
    }
    Ok(url)
}

async fn read_live_catalog() -> Result<String, String> {
    if let Ok(path) = std::env::var("BRAMA_MODEL_CATALOG_PATH") {
        return tokio::fs::read_to_string(path)
            .await
            .map_err(|error| error.to_string());
    }
    let url = catalog_url()?;
    // One client for the refresh, not one per refresh: each `Client` carries its
    // own connection pool, and a new one every cycle leaks the sockets of the
    // last one until the process runs out of descriptors.
    static CATALOG_CLIENT: std::sync::OnceLock<Result<reqwest::Client, String>> =
        std::sync::OnceLock::new();
    let client = CATALOG_CLIENT
        .get_or_init(|| {
            reqwest::Client::builder()
                .timeout(Duration::from_secs(30))
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .map_err(|error| error.to_string())
        })
        .clone()?;
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|error| error.to_string())?
        .error_for_status()
        .map_err(|error| error.to_string())?;
    response.text().await.map_err(|error| error.to_string())
}

fn cache_path() -> PathBuf {
    std::env::var("BRAMA_MODEL_CATALOG_CACHE")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(DEFAULT_CACHE_PATH))
}

async fn read_cache() -> Result<String, String> {
    tokio::fs::read_to_string(cache_path())
        .await
        .map_err(|error| error.to_string())
}

async fn write_cache(raw: &str) -> Result<(), String> {
    let path = cache_path();
    let temporary = path.with_extension(format!("tmp-{}", std::process::id()));
    tokio::fs::write(&temporary, raw)
        .await
        .map_err(|error| error.to_string())?;
    tokio::fs::rename(&temporary, &path)
        .await
        .map_err(|error| error.to_string())
}

fn parse_catalog(raw: &str) -> Result<CatalogSnapshot, String> {
    let root: Value = serde_json::from_str(raw).map_err(|error| error.to_string())?;
    let rows = root
        .as_object()
        .ok_or_else(|| "models.dev root must be an object".to_string())?;
    let mut providers = HashMap::with_capacity(rows.len());
    let mut models = Vec::new();
    let mut latest_update = String::new();

    for (catalog_key, row) in rows {
        let id = row
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or(catalog_key)
            .trim();
        if !valid_provider_id(id) {
            continue;
        }
        let npm = row.get("npm").and_then(Value::as_str).unwrap_or_default();
        let (protocol, auth) = protocol_for(npm);
        providers.insert(
            id.to_string(),
            CatalogProvider {
                id: id.to_string(),
                display_name: row
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or(id)
                    .to_string(),
                protocol,
                auth,
            },
        );

        let Some(model_rows) = row.get("models").and_then(Value::as_object) else {
            continue;
        };
        models.reserve(model_rows.len());
        for (model_key, model) in model_rows {
            let model_id = model
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or(model_key)
                .trim();
            if !valid_model_id(model_id) {
                continue;
            }
            if let Some(updated) = model.get("last_updated").and_then(Value::as_str) {
                if updated > latest_update.as_str() {
                    latest_update = updated.to_string();
                }
            }
            let input_modalities = model
                .pointer("/modalities/input")
                .and_then(Value::as_array)
                .map(|values| {
                    values
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect::<Vec<_>>()
                })
                .filter(|values| !values.is_empty())
                .unwrap_or_else(|| vec!["text".to_string()]);
            models.push(RegistryModel {
                route_id: format!("{id}/{model_id}"),
                provider_id: id.to_string(),
                model_id: model_id.to_string(),
                context_window: model
                    .pointer("/limit/context")
                    .and_then(Value::as_u64)
                    .unwrap_or(128_000),
                max_output_tokens: model
                    .pointer("/limit/output")
                    .and_then(Value::as_u64)
                    .unwrap_or(16_384),
                input_modalities,
                tools: model
                    .get("tool_call")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                reasoning: model
                    .get("reasoning")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                input_price: cost(model, "input"),
                output_price: cost(model, "output"),
                cache_read_price: cost(model, "cache_read"),
                cache_write_price: cost(model, "cache_write"),
            });
        }
    }

    models.sort_by(|left, right| left.route_id.cmp(&right.route_id));
    models.dedup_by(|left, right| left.route_id == right.route_id);
    let revision = format!(
        "models-dev-{latest_update}-{}-{}",
        providers.len(),
        models.len()
    );
    Ok(CatalogSnapshot {
        providers,
        models,
        revision,
    })
}

fn protocol_for(npm: &str) -> (CatalogProtocol, CatalogAuth) {
    if npm == "@ai-sdk/anthropic" {
        return (CatalogProtocol::AnthropicMessages, CatalogAuth::XApiKey);
    }
    if npm == "@ai-sdk/google" {
        return (
            CatalogProtocol::GoogleGenerateContent,
            CatalogAuth::GoogleApiKey,
        );
    }
    if npm == "@ai-sdk/openai-compatible"
        || matches!(
            npm,
            "@ai-sdk/openai"
                | "@ai-sdk/xai"
                | "@ai-sdk/mistral"
                | "@ai-sdk/cerebras"
                | "@ai-sdk/perplexity"
                | "@ai-sdk/deepinfra"
                | "@openrouter/ai-sdk-provider"
                | "@ai-sdk/togetherai"
                | "@ai-sdk/gateway"
                | "@ai-sdk/groq"
                | "venice-ai-sdk-provider"
                | "merge-gateway-ai-sdk-provider"
                | "ai-gateway-provider"
        )
    {
        return (CatalogProtocol::OpenAiChat, CatalogAuth::Bearer);
    }
    (CatalogProtocol::Unsupported, CatalogAuth::Bearer)
}

fn cost(model: &Value, key: &str) -> f64 {
    model
        .get("cost")
        .and_then(|cost| cost.get(key))
        .and_then(Value::as_f64)
        .unwrap_or(0.0)
}

fn valid_provider_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

fn valid_model_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 512
        && value.trim() == value
        && !value.chars().any(char::is_control)
}
