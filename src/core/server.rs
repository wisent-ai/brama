use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use axum::extract::{ConnectInfo, Extension, Path, State};
use axum::http::{header::AUTHORIZATION, HeaderMap, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use tracing::{info, warn};

use crate::subscription_dispatch::{
    authenticate_agent, dispatch_any_subscription, dispatch_any_vision_capable_subscription,
    dispatch_direct_openai_typed, dispatch_direct_with_fallback, dispatch_subscription,
    dispatch_task_subscription, is_subscription_model, provider_requires_caller_identity,
    registry_models_for_agent,
};
use crate::types::{BillingTarget, Message, ModelRequest, ModelResponse, Tool, ToolCall};

static TOTAL_REQUESTS: AtomicU64 = AtomicU64::new(0);
static TOTAL_INPUT_TOKENS: AtomicU64 = AtomicU64::new(0);
static TOTAL_OUTPUT_TOKENS: AtomicU64 = AtomicU64::new(0);
static TOTAL_PROVIDER_ATTEMPTS: AtomicU64 = AtomicU64::new(u64::MIN);
static TOTAL_FAILURES: AtomicU64 = AtomicU64::new(u64::MIN);
static STARTED_AT: LazyLock<Instant> = LazyLock::new(Instant::now);

fn max_output_tokens() -> u32 {
    "32768".parse().expect("valid output token limit")
}

fn max_temperature() -> f64 {
    "2".parse().expect("valid temperature limit")
}

fn request_deadline() -> Duration {
    Duration::from_secs("300".parse().expect("valid request deadline"))
}

const MODEL_ROUTER_CLIENT_IDENTITIES_ENV: &str = "BRAMA_MODEL_ROUTER_CLIENT_IDENTITIES";
const MODEL_ALIASES_ENV: &str = "BRAMA_MODEL_ALIASES";
const WISENT_CHAT_PRIMARY_ALIAS: &str = "wisent-backend/chat/primary";
const WISENT_CHAT_FALLBACK_ALIAS: &str = "wisent-backend/chat/fallback";
const WISENT_EVALUATION_ALIAS: &str = "wisent-backend/evaluation";
const WISENT_EMBEDDING_ALIAS: &str = "wisent-backend/embeddings";
const WISENT_MODERATION_ALIAS: &str = "wisent-backend/moderation";
const WELES_AGENT_PRIMARY_ALIAS: &str = "weles/agent/primary";
const WISENT_MODEL_ALIASES: &[&str] = &[
    WISENT_CHAT_PRIMARY_ALIAS,
    WISENT_CHAT_FALLBACK_ALIAS,
    WISENT_EVALUATION_ALIAS,
    WISENT_EMBEDDING_ALIAS,
    WISENT_MODERATION_ALIAS,
];
const MODEL_ALIASES: &[&str] = &[
    WISENT_CHAT_PRIMARY_ALIAS,
    WISENT_CHAT_FALLBACK_ALIAS,
    WISENT_EVALUATION_ALIAS,
    WISENT_EMBEDDING_ALIAS,
    WISENT_MODERATION_ALIAS,
    WELES_AGENT_PRIMARY_ALIAS,
];

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ModelClientCredential {
    client_id: String,
    token: String,
    #[serde(default)]
    agent_id: Option<String>,
    #[serde(default)]
    allowed_models: Option<Vec<String>>,
}

#[derive(Clone, Debug)]
struct ModelClientIdentity {
    client_id: String,
    agent_id: Option<String>,
    allowed_models: Option<HashSet<String>>,
}
impl ModelClientIdentity {
    fn authorizes_model(&self, model: &str) -> bool {
        self.allowed_models
            .as_ref()
            .is_none_or(|models| models.contains(model))
    }
}

#[derive(Clone)]
struct ModelIngressCredential {
    identity: ModelClientIdentity,
    token_digest: sha2::digest::Output<Sha256>,
}

#[derive(Clone)]
struct ModelIngressAuth {
    credentials: Vec<ModelIngressCredential>,
}

impl ModelIngressAuth {
    fn from_env() -> Result<Self, std::io::Error> {
        let encoded = std::env::var(MODEL_ROUTER_CLIENT_IDENTITIES_ENV).map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("{MODEL_ROUTER_CLIENT_IDENTITIES_ENV} is required"),
            )
        })?;
        let configured: Vec<ModelClientCredential> =
            serde_json::from_str(&encoded).map_err(|error| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!("{MODEL_ROUTER_CLIENT_IDENTITIES_ENV} is invalid: {error}"),
                )
            })?;
        if configured.is_empty() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("{MODEL_ROUTER_CLIENT_IDENTITIES_ENV} must not be empty"),
            ));
        }

        let mut client_ids = HashSet::with_capacity(configured.len());
        let mut credentials = Vec::with_capacity(configured.len());
        for credential in configured {
            if credential.client_id.is_empty()
                || credential.client_id.trim() != credential.client_id
                || !credential
                    .client_id
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
            {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!("{MODEL_ROUTER_CLIENT_IDENTITIES_ENV} contains an invalid client_id"),
                ));
            }
            if !client_ids.insert(credential.client_id.clone()) {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!(
                        "{MODEL_ROUTER_CLIENT_IDENTITIES_ENV} contains duplicate client_id {}",
                        credential.client_id
                    ),
                ));
            }
            if credential.token.is_empty()
                || credential.token.trim() != credential.token
                || credential
                    .token
                    .bytes()
                    .any(|byte| byte.is_ascii_control() || byte.is_ascii_whitespace())
            {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!(
                        "{MODEL_ROUTER_CLIENT_IDENTITIES_ENV} contains an invalid token for {}",
                        credential.client_id
                    ),
                ));
            }
            if credential.agent_id.as_deref().is_some_and(|agent_id| {
                agent_id.is_empty()
                    || agent_id.trim() != agent_id
                    || !agent_id.bytes().all(|byte| {
                        byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-'
                    })
            }) {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!(
                        "{MODEL_ROUTER_CLIENT_IDENTITIES_ENV} contains an invalid agent_id for {}",
                        credential.client_id
                    ),
                ));
            }
            let allowed_models = credential
                .allowed_models
                .map(|models| {
                    if models.is_empty()
                        || models.iter().any(|model| {
                            model.is_empty()
                                || model.trim() != model
                                || model.contains('*')
                                || model.bytes().any(|byte| byte.is_ascii_whitespace())
                        })
                    {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::InvalidInput,
                            format!(
                                "{MODEL_ROUTER_CLIENT_IDENTITIES_ENV} contains invalid allowed_models for {}",
                                credential.client_id
                            ),
                        ));
                    }
                    let model_count = models.len();
                    let unique = models.into_iter().collect::<HashSet<_>>();
                    if unique.len() != model_count {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::InvalidInput,
                            format!(
                                "{MODEL_ROUTER_CLIENT_IDENTITIES_ENV} contains duplicate allowed_models for {}",
                                credential.client_id
                            ),
                        ));
                    }
                    Ok(unique)
                })
                .transpose()?;

            let token_digest = Sha256::digest(credential.token.as_bytes());
            if credentials.iter().any(|existing: &ModelIngressCredential| {
                existing
                    .token_digest
                    .as_slice()
                    .ct_eq(token_digest.as_slice())
                    .into()
            }) {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!("{MODEL_ROUTER_CLIENT_IDENTITIES_ENV} contains duplicate tokens"),
                ));
            }
            credentials.push(ModelIngressCredential {
                identity: ModelClientIdentity {
                    client_id: credential.client_id,
                    agent_id: credential.agent_id,
                    allowed_models,
                },
                token_digest,
            });
        }
        Ok(Self { credentials })
    }

    fn requires_exact_aliases(
        &self,
        client_id: &str,
        aliases: &[&str],
    ) -> Result<(), std::io::Error> {
        let expected = aliases
            .iter()
            .copied()
            .map(str::to_string)
            .collect::<HashSet<_>>();
        let valid = self.credentials.iter().any(|credential| {
            credential.identity.client_id == client_id
                && credential.identity.allowed_models.as_ref() == Some(&expected)
        });
        if valid {
            Ok(())
        } else {
            Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!(
                    "{MODEL_ROUTER_CLIENT_IDENTITIES_ENV} must give `{client_id}` its exact required alias set"
                ),
            ))
        }
    }
    fn identity_for(&self, headers: &HeaderMap) -> Option<ModelClientIdentity> {
        let mut values = headers.get_all(AUTHORIZATION).iter();
        let value = values.next()?;
        if values.next().is_some() {
            return None;
        }
        let token = value
            .to_str()
            .ok()?
            .strip_prefix("Bearer ")
            .filter(|token| !token.is_empty())?;
        let presented_digest = Sha256::digest(token.as_bytes());
        let mut matched = None;
        for credential in &self.credentials {
            let equal: bool = credential
                .token_digest
                .as_slice()
                .ct_eq(presented_digest.as_slice())
                .into();
            if equal {
                matched = Some(credential.identity.clone());
            }
        }
        matched
    }
}

#[derive(Clone, Debug)]
struct ModelAliases {
    routes: HashMap<String, String>,
    routes_file: Option<PathBuf>,
}

impl ModelAliases {
    fn from_env() -> Result<Self, std::io::Error> {
        let encoded = std::env::var(MODEL_ALIASES_ENV).map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("{MODEL_ALIASES_ENV} is required"),
            )
        })?;
        let routes: HashMap<String, String> = serde_json::from_str(&encoded).map_err(|error| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("{MODEL_ALIASES_ENV} is invalid: {error}"),
            )
        })?;
        let routes_file = crate::core::inference_routes::configured_path();
        let mut effective_routes = routes.clone();
        let mut effective_fallbacks: HashMap<String, Vec<String>> = HashMap::new();
        if let Some(path) = routes_file.as_deref() {
            let dynamic = crate::core::inference_routes::resolved(path)
                .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidInput, error))?;
            effective_routes.extend(dynamic);
            effective_fallbacks = crate::core::inference_routes::resolved_fallbacks(path)
                .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidInput, error))?;
        }
        let expected = MODEL_ALIASES
            .iter()
            .copied()
            .map(str::to_string)
            .collect::<HashSet<_>>();
        if effective_routes.keys().cloned().collect::<HashSet<_>>() != expected {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("{MODEL_ALIASES_ENV} must contain the exact named alias set"),
            ));
        }
        if effective_fallbacks
            .keys()
            .any(|alias| !effective_routes.contains_key(alias))
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "inference fallback alias has no primary route",
            ));
        }
        for (alias, route) in effective_routes.iter().chain(
            effective_fallbacks
                .iter()
                .flat_map(|(alias, routes)| routes.iter().map(move |route| (alias, route))),
        ) {
            if route.contains('*') || route.trim() != route {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!("{MODEL_ALIASES_ENV} contains an unsafe route for {alias}"),
                ));
            }
            let supported = match alias.as_str() {
                WISENT_CHAT_PRIMARY_ALIAS
                | WISENT_CHAT_FALLBACK_ALIAS
                | WISENT_EVALUATION_ALIAS
                | WELES_AGENT_PRIMARY_ALIAS => {
                    crate::providers::adapter::supports_chat_route(route)
                }
                WISENT_EMBEDDING_ALIAS => {
                    crate::providers::adapter::supports_embedding_route(route)
                }
                WISENT_MODERATION_ALIAS => {
                    crate::providers::adapter::supports_moderation_route(route)
                }
                _ => false,
            };
            let provider = crate::providers::adapter::provider_id_from_route(route);
            if !supported
                || provider.is_none_or(|provider| {
                    !crate::gateway::broker::provider_capability_configured(provider)
                })
            {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!(
                        "{MODEL_ALIASES_ENV} route for {alias} has no configured matching provider capability"
                    ),
                ));
            }
        }
        Ok(Self {
            routes,
            routes_file,
        })
    }

    fn source(&self, alias: &str) -> Option<String> {
        if let Some(path) = self.routes_file.as_deref() {
            match crate::core::inference_routes::resolve(path, alias) {
                Ok(Some(route)) => return Some(route),
                Ok(None) => {}
                Err(error) => {
                    warn!(event = "inference_routes_invalid", %error);
                    return None;
                }
            }
        }
        self.routes.get(alias).cloned()
    }

    fn chat_route(&self, alias: &str) -> (Option<String>, Vec<String>) {
        if !matches!(
            alias,
            WISENT_CHAT_PRIMARY_ALIAS | WISENT_CHAT_FALLBACK_ALIAS | WISENT_EVALUATION_ALIAS
        ) {
            return (None, Vec::new());
        }
        if let Some(path) = self.routes_file.as_deref() {
            match crate::core::inference_routes::route_chain(path, alias) {
                Ok(Some(chain)) => {
                    let mut destinations = chain.into_iter();
                    return (destinations.next(), destinations.collect::<Vec<String>>());
                }
                Ok(None) => {}
                Err(error) => {
                    warn!(event = "inference_routes_invalid", %error);
                    return (None, Vec::new());
                }
            }
        }
        (self.routes.get(alias).cloned(), Vec::new())
    }
}

fn exact_agent_header(headers: &HeaderMap, expected: &str) -> bool {
    let mut values = headers.get_all("x-agent-id").iter();
    values
        .next()
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value == expected)
        && values.next().is_none()
}

async fn require_model_bearer(
    State(auth): State<ModelIngressAuth>,
    mut request: axum::extract::Request,
    next: Next,
) -> Response {
    let Some(identity) = auth.identity_for(request.headers()) else {
        return api_error(StatusCode::UNAUTHORIZED, "unauthorized").into_response();
    };
    if identity.allowed_models.is_some()
        && !matches!(
            request.uri().path(),
            "/v1/chat/completions" | "/v1/embeddings" | "/v1/moderations" | "/v1/models"
        )
    {
        return api_error(StatusCode::FORBIDDEN, "forbidden").into_response();
    }
    if let Some(agent_id) = identity.agent_id.as_deref() {
        if has_caller_auth_headers(request.headers())
            && !exact_agent_header(request.headers(), agent_id)
        {
            warn!(client_id = %identity.client_id, agent_id, "bearer identity does not match signed agent");
            return api_error(StatusCode::FORBIDDEN, "forbidden").into_response();
        }
    }
    request.extensions_mut().insert(identity);
    next.run(request).await
}

fn forwarded_proto_is_https(headers: &HeaderMap) -> Option<bool> {
    let forwarded = headers.get("forwarded").map(|value| {
        value.to_str().ok().is_some_and(|value| {
            value
                .split(',')
                .next()
                .into_iter()
                .flat_map(|entry| entry.split(';'))
                .filter_map(|part| part.trim().split_once('='))
                .find(|(name, _)| name.eq_ignore_ascii_case("proto"))
                .is_some_and(|(_, proto)| {
                    proto.trim().trim_matches('"').eq_ignore_ascii_case("https")
                })
        })
    });
    let x_forwarded = headers.get("x-forwarded-proto").map(|value| {
        value.to_str().ok().is_some_and(|value| {
            value
                .split(',')
                .next()
                .is_some_and(|proto| proto.trim().eq_ignore_ascii_case("https"))
        })
    });
    match (forwarded, x_forwarded) {
        (Some(left), Some(right)) => Some(left && right),
        (Some(value), None) | (None, Some(value)) => Some(value),
        (None, None) => None,
    }
}

fn trusted_forwarded_peer(peer: std::net::IpAddr) -> bool {
    std::env::var("BRAMA_TRUSTED_PROXY_IPS")
        .ok()
        .is_some_and(|configured| {
            configured
                .split(',')
                .filter_map(|value| value.trim().parse::<std::net::IpAddr>().ok())
                .any(|trusted| trusted == peer)
        })
}

async fn require_secure_transport(request: axum::extract::Request, next: Next) -> Response {
    let forwarded = forwarded_proto_is_https(request.headers());
    let peer = request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|connect_info| connect_info.0.ip());
    let loopback = peer.is_some_and(|address| address.is_loopback());
    let trusted_https_proxy = peer.is_some_and(trusted_forwarded_peer) && forwarded == Some(true);
    if loopback || trusted_https_proxy {
        return next.run(request).await;
    }
    api_error(
        StatusCode::UPGRADE_REQUIRED,
        "HTTPS is required except for direct loopback requests",
    )
    .into_response()
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ChatCompletionRequest {
    #[serde(default)]
    model: Option<String>,
    messages: Vec<ChatMessage>,
    #[serde(default = "default_max_tokens")]
    max_tokens: u32,
    #[serde(default = "default_temperature")]
    temperature: f64,
    #[serde(default)]
    tools: Option<Vec<Tool>>,
    #[serde(default, rename = "billingTarget")]
    billing_target: Option<BillingTarget>,
}

fn default_max_tokens() -> u32 {
    1024
}
fn default_temperature() -> f64 {
    0.7
}

fn is_any_subscription_selector(model: &str) -> bool {
    model.trim().eq_ignore_ascii_case("any")
}

fn is_any_vision_capable_subscription_selector(model: &str) -> bool {
    model.trim().eq_ignore_ascii_case("any-vision-capable")
}

fn task_subscription_selector(model: &str) -> Option<String> {
    model
        .trim()
        .strip_prefix("task:")
        .map(str::trim)
        .filter(|task| !task.is_empty())
        .map(String::from)
}

fn has_caller_auth_headers(headers: &HeaderMap) -> bool {
    ["x-agent-id", "x-agent-timestamp", "x-agent-signature"]
        .iter()
        .any(|name| headers.contains_key(*name))
}

#[derive(Debug, Deserialize)]
struct ChatMessage {
    role: String,
    #[serde(default)]
    content: Option<serde_json::Value>,
    #[serde(default)]
    tool_call_id: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    tool_calls: Option<Vec<serde_json::Value>>,
}

#[derive(Debug, Serialize)]
struct ChatCompletionResponse {
    id: String,
    object: String,
    model: String,
    choices: Vec<Choice>,
    usage: Usage,
}

#[derive(Debug, Serialize)]
struct Choice {
    index: u32,
    message: ChoiceMessage,
    finish_reason: String,
}

#[derive(Debug, Serialize)]
struct ChoiceMessage {
    role: String,
    content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_calls: Option<Vec<ToolCall>>,
}

#[derive(Debug, Serialize)]
struct Usage {
    prompt_tokens: u32,

    completion_tokens: u32,
    total_tokens: u32,
}

#[derive(Clone, Copy)]
struct ModelErrorContract {
    status: StatusCode,
    error_type: &'static str,
    code: &'static str,
    retryable: bool,
}

fn model_error_contract(message: &str) -> ModelErrorContract {
    let normalized = message.to_ascii_lowercase();
    if normalized.starts_with("provider_rate_limited:") {
        return ModelErrorContract {
            status: StatusCode::TOO_MANY_REQUESTS,
            error_type: "capacity_error",
            code: "provider_rate_limited",
            retryable: true,
        };
    }
    if normalized.starts_with("dependency_timeout:") {
        return ModelErrorContract {
            status: StatusCode::GATEWAY_TIMEOUT,
            error_type: "dependency_error",
            code: "dependency_timeout",
            retryable: true,
        };
    }
    if normalized.starts_with("dependency_unavailable:") {
        return ModelErrorContract {
            status: StatusCode::SERVICE_UNAVAILABLE,
            error_type: "dependency_error",
            code: "dependency_unavailable",
            retryable: true,
        };
    }
    if normalized.starts_with("provider_failure:")
        || normalized.starts_with("provider_authentication:")
    {
        return ModelErrorContract {
            status: StatusCode::BAD_GATEWAY,
            error_type: "provider_error",
            code: "provider_failure",
            retryable: false,
        };
    }
    if normalized.starts_with("auth:")
        || normalized.contains("missing x-agent-")
        || normalized.contains("no auth secret for agent")
    {
        return ModelErrorContract {
            status: StatusCode::UNAUTHORIZED,
            error_type: "authentication_error",
            code: "unauthenticated",
            retryable: false,
        };
    }
    if normalized.contains("rate_limit")
        || normalized.contains("429")
        || normalized.contains("hit your limit")
        || normalized.contains("usage limit")
        || normalized.contains("weekly limit")
    {
        return ModelErrorContract {
            status: StatusCode::TOO_MANY_REQUESTS,
            error_type: "capacity_error",
            code: "provider_rate_limited",
            retryable: true,
        };
    }
    if normalized.contains("no active")
        || normalized.contains("no working")
        || normalized.contains("all bounded")
        || normalized.contains("selected credential")
    {
        return ModelErrorContract {
            status: StatusCode::TOO_MANY_REQUESTS,
            error_type: "capacity_error",
            code: "subscription_unavailable",
            retryable: true,
        };
    }
    if normalized.contains("deadline") || normalized.contains("timed out") {
        return ModelErrorContract {
            status: StatusCode::GATEWAY_TIMEOUT,
            error_type: "dependency_error",
            code: "dependency_timeout",
            retryable: true,
        };
    }
    if normalized.contains("unavailable")
        || normalized.contains("catalog")
        || normalized.contains("no provider")
    {
        return ModelErrorContract {
            status: StatusCode::SERVICE_UNAVAILABLE,
            error_type: "dependency_error",
            code: "dependency_unavailable",
            retryable: true,
        };
    }
    if normalized.contains("unknown provider/model")
        || normalized.contains("invalid provider/model")
        || normalized.contains("unsupported selector")
        || normalized.contains("no quality checks")
    {
        return ModelErrorContract {
            status: StatusCode::BAD_REQUEST,
            error_type: "request_error",
            code: "invalid_request",
            retryable: false,
        };
    }
    ModelErrorContract {
        status: StatusCode::BAD_GATEWAY,
        error_type: "provider_error",
        code: "provider_failure",
        retryable: false,
    }
}

fn typed_dispatch_attempts(contract: ModelErrorContract) -> u32 {
    if contract.code == "dependency_unavailable" {
        u32::default()
    } else {
        u32::from(true)
    }
}

fn record_typed_request(attempts: u32, failed: bool) {
    TOTAL_REQUESTS.fetch_add(u64::from(true), Ordering::Relaxed);
    TOTAL_PROVIDER_ATTEMPTS.fetch_add(attempts as u64, Ordering::Relaxed);
    if failed {
        TOTAL_FAILURES.fetch_add(u64::from(true), Ordering::Relaxed);
    }
}
fn typed_usage_tokens(body: &Value, keys: &[&str]) -> u64 {
    let Some(usage) = body.get("usage").and_then(Value::as_object) else {
        return u64::default();
    };
    keys.iter()
        .find_map(|key| usage.get(*key).and_then(Value::as_u64))
        .unwrap_or_default()
}

fn record_typed_usage(body: &Value) {
    TOTAL_INPUT_TOKENS.fetch_add(
        typed_usage_tokens(body, &["prompt_tokens", "input_tokens"]),
        Ordering::Relaxed,
    );
    TOTAL_OUTPUT_TOKENS.fetch_add(
        typed_usage_tokens(body, &["completion_tokens", "output_tokens"]),
        Ordering::Relaxed,
    );
}

fn typed_dispatch_error(message: &str) -> ApiError {
    let contract = model_error_contract(message);
    let attempts = typed_dispatch_attempts(contract);
    error_response(
        contract.status,
        contract.error_type,
        contract.code,
        message,
        contract.retryable,
        attempts,
    )
}

async fn chat_completions(
    Extension(client_identity): Extension<ModelClientIdentity>,
    Extension(aliases): Extension<ModelAliases>,
    headers: axum::http::HeaderMap,
    body: axum::body::Bytes,
) -> impl IntoResponse {
    let req: ChatCompletionRequest = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(error) => {
            return api_error(StatusCode::BAD_REQUEST, &format!("invalid JSON: {error}"));
        }
    };
    if req.messages.is_empty() {
        return api_error(StatusCode::BAD_REQUEST, "messages must not be empty");
    }
    if req.max_tokens == u32::default() || req.max_tokens > max_output_tokens() {
        return api_error(
            StatusCode::BAD_REQUEST,
            &format!("max_tokens must be between one and {}", max_output_tokens()),
        );
    }
    if !req.temperature.is_finite()
        || req.temperature < f64::default()
        || req.temperature > max_temperature()
    {
        return api_error(
            StatusCode::BAD_REQUEST,
            &format!(
                "temperature must be finite and between zero and {}",
                max_temperature()
            ),
        );
    }

    let messages: Vec<Message> = req
        .messages
        .into_iter()
        .map(|m| Message {
            role: m.role,
            content: m
                .content
                .unwrap_or_else(|| serde_json::Value::String(String::new())),
            tool_call_id: m.tool_call_id,
            name: m.name,
            tool_calls: m.tool_calls,
        })
        .collect();

    let requested_model = req.model.as_deref().unwrap_or("").trim();
    if requested_model.is_empty() {
        return api_error(StatusCode::BAD_REQUEST, "missing field `model`");
    }
    let any_subscription = is_any_subscription_selector(requested_model);
    let any_vision_capable_subscription =
        is_any_vision_capable_subscription_selector(requested_model);
    if !client_identity.authorizes_model(requested_model) {
        return api_error(StatusCode::FORBIDDEN, "forbidden");
    }
    let task_subscription = task_subscription_selector(requested_model);
    let (alias_source, alias_fallbacks) = aliases.chat_route(requested_model);
    let selected_model = alias_source
        .as_deref()
        .unwrap_or(requested_model)
        .to_string();
    let canonical_model = is_subscription_model(&selected_model);
    if !(any_subscription
        || any_vision_capable_subscription
        || task_subscription.is_some()
        || canonical_model)
    {
        return api_error(
            StatusCode::BAD_REQUEST,
            "model must be a canonical provider/model route or a supported selector",
        );
    }
    let caller_scoped_request = any_subscription
        || any_vision_capable_subscription
        || task_subscription.is_some()
        || req.billing_target.is_some()
        || provider_requires_caller_identity(&selected_model)
        || has_caller_auth_headers(&headers);

    let routing_mode = if task_subscription.is_some() {
        "task"
    } else if any_vision_capable_subscription {
        "any-vision-capable"
    } else if any_subscription {
        "any"
    } else if caller_scoped_request {
        "subscription"
    } else if alias_source.is_some() {
        "alias"
    } else {
        "direct"
    };
    let request_id = format!("chatcmpl-{}", uuid_v4());
    info!(
        event = "routing_decision",
        request_id = %request_id,
        client_id = %client_identity.client_id,
        agent_id = client_identity.agent_id.as_deref().unwrap_or("none"),
        routing_mode,
        requested_model,
        selected_model = %selected_model,
        "routing request accepted"
    );
    let dispatch_started = Instant::now();
    let dispatch_deadline = request_deadline().saturating_mul(
        u32::try_from(alias_fallbacks.len().saturating_add(usize::from(true))).unwrap_or(u32::MAX),
    );
    let mut resp = match tokio::time::timeout(dispatch_deadline, async {
        // Preserve the OpenAI system-role message as ModelRequest.system while
        // the stateless provider adapter receives the remaining conversation.
        let (system, non_system_messages): (Option<String>, Vec<Message>) = {
            let mut sys: Option<String> = None;
            let mut rest: Vec<Message> = Vec::with_capacity(messages.len());
            for m in messages {
                if m.role == "system" && sys.is_none() {
                    sys = m.content.as_str().map(|s| s.to_string());
                    continue;
                }
                rest.push(m);
            }
            (sys, rest)
        };
        let dispatch_req = ModelRequest {
            messages: non_system_messages,
            model: selected_model.clone(),
            max_tokens: req.max_tokens,
            temperature: req.temperature,
            system,
            tools: req.tools,
            billing_target: req.billing_target,
        };
        if let Some(task) = task_subscription.as_deref() {
            dispatch_task_subscription(&headers, &dispatch_req, &body, task).await
        } else if any_vision_capable_subscription {
            dispatch_any_vision_capable_subscription(&headers, &dispatch_req, &body).await
        } else if any_subscription {
            dispatch_any_subscription(&headers, &dispatch_req, &body).await
        } else if caller_scoped_request {
            dispatch_subscription(&headers, &dispatch_req, &body).await
        } else {
            dispatch_direct_with_fallback(&dispatch_req, &alias_fallbacks).await
        }
    })
    .await
    {
        Ok(response) => response,
        Err(_) => ModelResponse::failure(&selected_model, "whole request deadline exceeded".into()),
    };
    if resp.latency_ms == f64::default() {
        resp.latency_ms = dispatch_started.elapsed().as_millis() as f64;
    }

    TOTAL_REQUESTS.fetch_add(1, Ordering::Relaxed);
    TOTAL_INPUT_TOKENS.fetch_add(resp.input_tokens as u64, Ordering::Relaxed);
    TOTAL_OUTPUT_TOKENS.fetch_add(resp.output_tokens as u64, Ordering::Relaxed);
    TOTAL_PROVIDER_ATTEMPTS.fetch_add(resp.attempts as u64, Ordering::Relaxed);
    if !resp.success {
        TOTAL_FAILURES.fetch_add(u64::from(true), Ordering::Relaxed);
    }
    let failure_contract = resp.error.as_deref().map(model_error_contract);
    info!(
        event = "routing_complete",
        request_id = %request_id,
        client_id = %client_identity.client_id,
        routing_mode,
        requested_model,
        selected_model = %resp.model,
        attempts = resp.attempts,
        success = resp.success,
        elapsed_ms = dispatch_started.elapsed().as_millis() as u64,
        error_code = failure_contract.map(|contract| contract.code).unwrap_or("none"),
        retryable = failure_contract.is_some_and(|contract| contract.retryable),
        input_tokens = resp.input_tokens,
        output_tokens = resp.output_tokens,
        operator_action_required = failure_contract.is_some_and(|contract| !contract.retryable),
        "routing request completed"
    );
    if !resp.success {
        let message = resp.error.take().unwrap_or_default();
        let contract = model_error_contract(&message);
        return error_response(
            contract.status,
            contract.error_type,
            contract.code,
            &message,
            contract.retryable,
            resp.attempts,
        );
    }

    // Alias telemetry stays attached to the stable logical selector.
    crate::core::perf::record(requested_model, resp.latency_ms, resp.output_tokens);

    let has_tool_calls = resp.tool_calls.as_ref().map_or(false, |tc| !tc.is_empty());
    let finish_reason = if has_tool_calls { "tool_calls" } else { "stop" };

    let response_model = alias_source
        .map(|_| requested_model.to_string())
        .unwrap_or_else(|| resp.model.clone());
    let body = serde_json::to_value(ChatCompletionResponse {
        id: request_id,
        object: "chat.completion".into(),
        model: response_model,
        choices: vec![Choice {
            index: 0,
            message: ChoiceMessage {
                role: "assistant".into(),
                content: resp.content,
                tool_calls: resp.tool_calls,
            },
            finish_reason: finish_reason.into(),
        }],
        usage: Usage {
            prompt_tokens: resp.input_tokens,
            completion_tokens: resp.output_tokens,
            total_tokens: resp.input_tokens + resp.output_tokens,
        },
    })
    .unwrap_or_default();

    (StatusCode::OK, Json(body))
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(untagged)]
enum TextInput {
    One(String),
    Many(Vec<String>),
}

impl TextInput {
    fn is_valid(&self) -> bool {
        match self {
            Self::One(value) => !value.is_empty(),
            Self::Many(values) => {
                !values.is_empty() && values.iter().all(|value| !value.is_empty())
            }
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct EmbeddingRequest {
    model: String,
    input: TextInput,
    #[serde(default)]
    encoding_format: Option<String>,
    #[serde(default)]
    dimensions: Option<u32>,
    #[serde(default)]
    user: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ModerationRequest {
    model: String,
    input: TextInput,
}

async fn embeddings(
    Extension(client_identity): Extension<ModelClientIdentity>,
    Extension(aliases): Extension<ModelAliases>,
    Json(request): Json<EmbeddingRequest>,
) -> Result<Json<Value>, ApiError> {
    if request.model != WISENT_EMBEDDING_ALIAS
        || !client_identity.authorizes_model(&request.model)
        || !request.input.is_valid()
        || request
            .dimensions
            .is_some_and(|value| value == u32::default())
    {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "invalid embedding request",
        ));
    }
    let source = aliases
        .source(WISENT_EMBEDDING_ALIAS)
        .ok_or_else(|| api_error(StatusCode::INTERNAL_SERVER_ERROR, "embedding alias missing"))?;
    let mut payload = Map::new();
    payload.insert(
        "input".to_string(),
        serde_json::to_value(request.input)
            .map_err(|_| api_error(StatusCode::BAD_REQUEST, "invalid embedding input"))?,
    );
    if let Some(value) = request.encoding_format {
        if !matches!(value.as_str(), "float" | "base64") {
            return Err(api_error(
                StatusCode::BAD_REQUEST,
                "invalid embedding encoding format",
            ));
        }
        payload.insert("encoding_format".to_string(), Value::String(value));
    }
    if let Some(value) = request.dimensions {
        payload.insert("dimensions".to_string(), json!(value));
    }
    if let Some(value) = request.user {
        if value.is_empty() {
            return Err(api_error(StatusCode::BAD_REQUEST, "invalid embedding user"));
        }
        payload.insert("user".to_string(), Value::String(value));
    }
    let body = match dispatch_direct_openai_typed(&source, "/v1/embeddings", payload).await {
        Ok(body) => body,
        Err(message) => {
            let attempts = typed_dispatch_attempts(model_error_contract(&message));
            record_typed_request(attempts, true);
            return Err(typed_dispatch_error(&message));
        }
    };
    record_typed_usage(&body);
    if !body.get("data").is_some_and(Value::is_array) {
        record_typed_request(u32::from(true), true);
        return Err(typed_dispatch_error(
            "embedding provider returned malformed data",
        ));
    }
    record_typed_request(u32::from(true), false);
    Ok(Json(body))
}

async fn moderations(
    Extension(client_identity): Extension<ModelClientIdentity>,
    Extension(aliases): Extension<ModelAliases>,
    Json(request): Json<ModerationRequest>,
) -> Result<Json<Value>, ApiError> {
    if request.model != WISENT_MODERATION_ALIAS
        || !client_identity.authorizes_model(&request.model)
        || !request.input.is_valid()
    {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "invalid moderation request",
        ));
    }
    let source = aliases.source(WISENT_MODERATION_ALIAS).ok_or_else(|| {
        api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "moderation alias missing",
        )
    })?;
    let mut payload = Map::new();
    payload.insert(
        "input".to_string(),
        serde_json::to_value(request.input)
            .map_err(|_| api_error(StatusCode::BAD_REQUEST, "invalid moderation input"))?,
    );
    let body = match dispatch_direct_openai_typed(&source, "/v1/moderations", payload).await {
        Ok(body) => body,
        Err(message) => {
            let attempts = typed_dispatch_attempts(model_error_contract(&message));
            record_typed_request(attempts, true);
            return Err(typed_dispatch_error(&message));
        }
    };
    record_typed_usage(&body);
    if !body.get("results").is_some_and(Value::is_array) {
        record_typed_request(u32::from(true), true);
        return Err(typed_dispatch_error(
            "moderation provider returned malformed data",
        ));
    }
    record_typed_request(u32::from(true), false);
    Ok(Json(body))
}

async fn health() -> impl IntoResponse {
    Json(json!({
        "status": "ok",
        "build": crate::build_info::current(),
        "dependencies": "not_probed",
    }))
}

/// Optional per-model telemetry block, present only when the route has stats.
fn perf_json(model: &str) -> Option<serde_json::Value> {
    crate::core::perf::get(model).map(|perf| {
        json!({
            "count": perf.count,
            "latencyMs": perf.latency_ms,
            "tps": perf.tps,
            "lastLatencyMs": perf.last_latency_ms,
            "lastTps": perf.last_tps,
        })
    })
}

async fn list_models(
    Extension(client_identity): Extension<ModelClientIdentity>,
    Extension(aliases): Extension<ModelAliases>,
    headers: axum::http::HeaderMap,
) -> Result<Json<serde_json::Value>, ApiError> {
    let catalog_agent = if has_caller_auth_headers(&headers) {
        Some(authorize_caller(&client_identity, &headers, &[], None).await?)
    } else {
        None
    };
    let include_perf = catalog_agent.is_some();
    let mut model_ids = Vec::new();
    let mut available = HashSet::new();

    let mut registry_metadata = HashMap::new();
    let mut catalog_revision =
        std::env::var("BRAMA_CATALOG_REVISION").unwrap_or_else(|_| "brama-v1".into());
    let mut degraded = false;
    match crate::subscription_dispatch::model_catalog::snapshot().await {
        Ok(catalog) => {
            catalog_revision = catalog.revision.clone();
            for model in &catalog.models {
                model_ids.push(model.route_id.clone());
                if catalog_agent.is_some()
                    && crate::gateway::broker::provider_capability_configured(&model.provider_id)
                {
                    available.insert(model.route_id.clone());
                }
                registry_metadata.insert(model.route_id.clone(), model.clone());
            }
        }
        Err(error) => {
            degraded = true;
            warn!(%error, "public model catalog unavailable");
        }
    }
    if let Some(catalog_agent) = catalog_agent.as_deref() {
        match registry_models_for_agent(catalog_agent).await {
            Ok(models) => {
                for model in models {
                    available.insert(model.route_id.clone());
                    model_ids.push(model.route_id.clone());
                    registry_metadata.insert(model.route_id.clone(), model);
                }
            }
            Err(error) => {
                degraded = true;
                warn!(%error, "native provider model discovery failed");
            }
        }
    }
    for alias in MODEL_ALIASES {
        if client_identity.authorizes_model(alias) && aliases.source(alias).is_some() {
            model_ids.push((*alias).to_string());
            available.insert((*alias).to_string());
        }
    }
    model_ids.sort();
    model_ids.dedup();
    model_ids.retain(|model| client_identity.authorizes_model(model));

    if headers.contains_key("x-jeden-schema-min") {
        let models = model_ids
            .into_iter()
            .map(|id| {
                let registry = registry_metadata.get(&id);
                let input_modalities = registry
                    .map(|model| model.input_modalities.clone())
                    .filter(|modalities| !modalities.is_empty())
                    .unwrap_or_else(|| vec!["text".to_string()]);
                let context_window = registry.map_or(200_000, |model| model.context_window);
                let max_output_tokens = registry.map_or(32_000, |model| model.max_output_tokens);
                let tools = registry.is_some_and(|model| model.tools);
                let reasoning = registry.is_some_and(|model| model.reasoning);
                let price = registry.map_or((0.0, 0.0, 0.0, 0.0), |model| {
                    (
                        model.input_price,
                        model.output_price,
                        model.cache_read_price,
                        model.cache_write_price,
                    )
                });
                let mut entry = json!({
                    "id": id,
                    "available": available.contains(&id),
                    "contextWindow": context_window,
                    "maxOutputTokens": max_output_tokens,
                    "inputModalities": input_modalities,
                    "outputModalities": ["text"],
                    "tools": tools,
                    "reasoning": reasoning,
                    "price": {
                        "input": price.0,
                        "output": price.1,
                        "cacheRead": price.2,
                        "cacheWrite": price.3,
                    },
                    "fallback": [],
                    "promotion": [],
                });
                if include_perf {
                    if let Some(perf) = perf_json(&id) {
                        entry["perf"] = perf;
                    }
                }
                entry
            })
            .collect::<Vec<_>>();
        return Ok(Json(json!({
            "catalogRevision": catalog_revision,
            "version": "v1",
            "models": models,
            "degraded": degraded,
        })));
    }
    let models = model_ids
        .into_iter()
        .map(|id| {
            let owner = registry_metadata
                .get(&id)
                .map(|model| model.provider_id.as_str())
                .unwrap_or("brama");
            let mut entry = json!({
                "id": id,
                "object": "model",
                "owned_by": owner,
            });
            if include_perf {
                if let Some(perf) = perf_json(&id) {
                    entry["perf"] = perf;
                }
            }
            entry
        })
        .collect::<Vec<_>>();
    Ok(Json(json!({
        "object": "list",
        "data": models,
    })))
}

fn wire_protocol_name(protocol: crate::providers::adapter::WireProtocol) -> &'static str {
    use crate::providers::adapter::WireProtocol;

    match protocol {
        WireProtocol::OpenAiChat => "openai-chat",
        WireProtocol::AnthropicMessages => "anthropic-messages",
        WireProtocol::OpenAiResponses => "openai-responses",
    }
}

async fn get_stats() -> impl IntoResponse {
    let provider_descriptors = crate::providers::adapter::providers();
    let configured_direct_providers = provider_descriptors
        .iter()
        .filter(|provider| crate::gateway::broker::provider_capability_configured(provider.id))
        .count();
    let providers = provider_descriptors
        .iter()
        .map(|provider| {
            json!({
                "id": provider.id,
                "displayName": provider.display_name,
                "wireProtocol": wire_protocol_name(provider.wire),
                "configured": crate::gateway::broker::provider_capability_configured(provider.id),
            })
        })
        .collect::<Vec<_>>();
    let models = crate::core::perf::snapshot()
        .into_iter()
        .map(|model| {
            json!({
                "model": model.model,
                "count": model.count,
                "latencyMs": model.latency_ms,
                "tps": model.tps,
                "lastLatencyMs": model.last_latency_ms,
                "lastTps": model.last_tps,
            })
        })
        .collect::<Vec<_>>();

    Json(json!({
        "build": crate::build_info::current(),
        "total_requests": TOTAL_REQUESTS.load(Ordering::Relaxed),
        "total_failures": TOTAL_FAILURES.load(Ordering::Relaxed),
        "total_provider_attempts": TOTAL_PROVIDER_ATTEMPTS.load(Ordering::Relaxed),
        "total_input_tokens": TOTAL_INPUT_TOKENS.load(Ordering::Relaxed),
        "total_output_tokens": TOTAL_OUTPUT_TOKENS.load(Ordering::Relaxed),
        "perfModels": models.len(),
        "configuredDirectProviders": configured_direct_providers,
        "uptimeSeconds": STARTED_AT.elapsed().as_secs(),
        "providers": providers,
        "models": models,
        "limits": {
            "maxOutputTokens": max_output_tokens(),
            "requestDeadlineSeconds": request_deadline().as_secs(),
        },
        "dependencyPolicy": {
            "catalog": "lazy",
            "capabilityBroker": "final-use",
            "subscriptions": "lazy",
        },
    }))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DonateSubscriptionRequest {
    provider: Option<String>,
    label: Option<String>,
    api_key: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RetireSubscriptionRequest {
    subscription_id: Option<String>,
}

type ApiError = (StatusCode, Json<serde_json::Value>);

fn error_response(
    status: StatusCode,
    error_type: &str,
    code: &str,
    message: &str,
    retryable: bool,
    attempts: u32,
) -> ApiError {
    (
        status,
        Json(json!({
            "error": {
                "message": message,
                "type": error_type,
                "code": code,
                "retryable": retryable,
                "attempts": attempts,
            }
        })),
    )
}

fn api_error(status: StatusCode, message: &str) -> ApiError {
    let (error_type, code, retryable) = match status {
        StatusCode::BAD_REQUEST => ("request_error", "invalid_request", false),
        StatusCode::UNAUTHORIZED => ("authentication_error", "unauthenticated", false),
        StatusCode::FORBIDDEN => ("authorization_error", "forbidden", false),
        StatusCode::NOT_FOUND => ("state_error", "subscription_not_found", false),
        StatusCode::CONFLICT => ("state_error", "state_conflict", false),
        StatusCode::UPGRADE_REQUIRED => ("transport_error", "secure_transport_required", false),
        StatusCode::TOO_MANY_REQUESTS => ("capacity_error", "subscription_unavailable", true),
        StatusCode::SERVICE_UNAVAILABLE => ("dependency_error", "dependency_unavailable", true),
        StatusCode::GATEWAY_TIMEOUT => ("dependency_error", "dependency_timeout", true),
        StatusCode::BAD_GATEWAY => ("provider_error", "provider_failure", false),
        _ => ("internal_error", "internal_error", false),
    };
    error_response(status, error_type, code, message, retryable, u32::default())
}

/// Authenticate the signed caller and, for agent-scoped resources, bind the
/// caller identity to the exact path identity. Request bodies are verified as
/// received so subscription mutations cannot substitute an unsigned donor.
async fn authorize_caller(
    client_identity: &ModelClientIdentity,
    headers: &axum::http::HeaderMap,
    raw_body: &[u8],
    target_agent_id: Option<&str>,
) -> Result<String, ApiError> {
    let caller = authenticate_agent(headers, raw_body)
        .await
        .map_err(|_| api_error(StatusCode::UNAUTHORIZED, "unauthorized"))?;
    if client_identity
        .agent_id
        .as_deref()
        .is_some_and(|bound| bound != caller.as_str())
        || target_agent_id.is_some_and(|target| target != caller.as_str())
    {
        return Err(api_error(StatusCode::FORBIDDEN, "forbidden"));
    }
    Ok(caller)
}

async fn list_agent_subscriptions(
    Extension(client_identity): Extension<ModelClientIdentity>,
    headers: axum::http::HeaderMap,
    Path(agent_id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    authorize_caller(&client_identity, &headers, &[], Some(&agent_id)).await?;
    let subscriptions = crate::gateway::broker::list_subscriptions(&agent_id)
        .await
        .into_iter()
        .map(|entry| {
            json!({
                "id": entry.id,
                "provider": entry.provider,
                "status": entry.status,
            })
        })
        .collect::<Vec<_>>();
    Ok(Json(json!({"subscriptions": subscriptions})))
}

async fn donate_subscription(
    Extension(client_identity): Extension<ModelClientIdentity>,
    headers: axum::http::HeaderMap,
    Path(agent_id): Path<String>,
    body: axum::body::Bytes,
) -> ApiError {
    if let Err(error) = authorize_caller(&client_identity, &headers, &body, Some(&agent_id)).await {
        return error;
    }
    let req: DonateSubscriptionRequest = match serde_json::from_slice(&body) {
        Ok(req) => req,
        Err(error) => {
            return api_error(
                StatusCode::BAD_REQUEST,
                &format!("invalid subscription request: {error}"),
            )
        }
    };
    let provider = match req.provider.as_deref().map(str::trim) {
        Some("claude_code" | "claude-code") => "claude-code",
        _ => return api_error(StatusCode::BAD_REQUEST, "provider must be claude_code"),
    };
    let api_key = req.api_key.as_deref().unwrap_or("");
    if api_key.is_empty() || api_key.chars().count() > 8000 {
        return api_error(
            StatusCode::BAD_REQUEST,
            "api_key must contain 1..8000 characters",
        );
    }
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let subscription_id = format!(
        "brama-sub-{}-claude-{millis}",
        crate::gateway::broker::slug(&agent_id)
    );
    if let Err(message) =
        crate::gateway::broker::put_donated_credential(&subscription_id, api_key).await
    {
        return api_error(StatusCode::INTERNAL_SERVER_ERROR, &message);
    }
    if let Err(message) = crate::gateway::broker::donated_add(&agent_id, &subscription_id, provider)
    {
        return api_error(StatusCode::INTERNAL_SERVER_ERROR, &message);
    }
    (
        StatusCode::OK,
        Json(json!({
            "subscription": {
                "id": subscription_id,
                "provider": provider,
                "agent_id": agent_id,
                "status": "active",
                "label": req.label,
            }
        })),
    )
}

async fn retire_subscription(
    Extension(client_identity): Extension<ModelClientIdentity>,
    headers: axum::http::HeaderMap,
    Path(agent_id): Path<String>,
    body: axum::body::Bytes,
) -> ApiError {
    if let Err(error) = authorize_caller(&client_identity, &headers, &body, Some(&agent_id)).await {
        return error;
    }
    let req: RetireSubscriptionRequest = match serde_json::from_slice(&body) {
        Ok(req) => req,
        Err(error) => {
            return api_error(
                StatusCode::BAD_REQUEST,
                &format!("invalid subscription request: {error}"),
            )
        }
    };
    let subscription_id = req.subscription_id.as_deref().map(str::trim).unwrap_or("");
    if subscription_id.is_empty() {
        return api_error(StatusCode::BAD_REQUEST, "subscription_id is required");
    }
    let owned = crate::gateway::broker::list_subscriptions(&agent_id)
        .await
        .into_iter()
        .any(|entry| entry.id == subscription_id);
    if !owned {
        return api_error(StatusCode::NOT_FOUND, "subscription not found");
    }
    crate::journal::retire(subscription_id);
    if let Err(message) = crate::gateway::broker::donated_remove(subscription_id) {
        return api_error(StatusCode::INTERNAL_SERVER_ERROR, &message);
    }
    (StatusCode::OK, Json(json!({"ok": true})))
}

pub async fn start_server(port: u16) -> Result<(), std::io::Error> {
    let _ = STARTED_AT.elapsed();
    let ingress_auth = ModelIngressAuth::from_env()?;
    ingress_auth.requires_exact_aliases("wisent-backend", WISENT_MODEL_ALIASES)?;
    ingress_auth.requires_exact_aliases("weles", &[WELES_AGENT_PRIMARY_ALIAS])?;
    let aliases = ModelAliases::from_env()?;
    // Touch the perf registry so persisted stats load at startup, not on first use.
    info!(
        models = crate::core::perf::tracked_count(),
        "perf registry loaded"
    );

    let protected = Router::new()
        .route("/v1/chat/completions", post(chat_completions))
        .route("/v1/embeddings", post(embeddings))
        .route("/v1/moderations", post(moderations))
        .route("/v1/models", get(list_models))
        .route(
            "/v1/subscriptions/:agent_id",
            get(list_agent_subscriptions)
                .post(donate_subscription)
                .delete(retire_subscription),
        )
        .route("/stats", get(get_stats))
        .layer(Extension(aliases))
        .layer(middleware::from_fn_with_state(
            ingress_auth,
            require_model_bearer,
        ));
    let app = Router::new()
        .route("/health", get(health))
        .merge(protected)
        .layer(middleware::from_fn(require_secure_transport));

    let addr = format!("0.0.0.0:{port}");
    info!("Starting brama server on {addr}");

    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await?;
    Ok(())
}

fn uuid_v4() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let t = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("{t:032x}")
}
