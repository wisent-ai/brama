use std::collections::{HashMap, HashSet};
use std::net::{Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use axum::extract::{ConnectInfo, Extension, Path, State};
use axum::http::{header::AUTHORIZATION, HeaderMap, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};
use futures_util::future::join_all;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use tracing::{info, warn};

use crate::core::failure::{self, IMPACT_MODEL_REQUEST, POINT_MODEL_REQUEST};
use crate::providers::stream::{StreamDelta, StreamItem};
use crate::subscription_dispatch::{
    authenticate_agent, dispatch_any_subscription, dispatch_any_subscription_stream,
    dispatch_any_vision_capable_subscription, dispatch_any_vision_capable_subscription_stream,
    dispatch_best_subscription, dispatch_best_subscription_for_agent,
    dispatch_best_subscription_stream, dispatch_best_subscription_stream_for_agent,
    dispatch_direct_openai_typed, dispatch_direct_with_fallback,
    dispatch_direct_with_fallback_stream, dispatch_subscription, dispatch_subscription_for_agent,
    dispatch_subscription_stream, dispatch_subscription_stream_for_agent,
    dispatch_task_subscription, dispatch_task_subscription_stream, is_subscription_model,
    provider_requires_caller_identity, registry_models_for_agent, RoutedStream,
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
pub const BEST_ALIAS: &str = "best";
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
    BEST_ALIAS,
];
const BRAMA_DESKTOP_CLIENT_ID: &str = "brama-desktop";
const BRAMA_USER_CLIENT_ID: &str = "brama-user";

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
    user_id: Option<String>,
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
        // Absent means "ask the authority", not "misconfigured". Building this
        // table requires reading every client's secret before the first request
        // is served, which is a broad grant for a router and the reason a
        // gateway could not start on a host that was not provisioned to decrypt
        // other clients' items. A bearer that is not here is resolved against
        // Skarbiec, which is where it was issued.
        let Ok(encoded) = std::env::var(MODEL_ROUTER_CLIENT_IDENTITIES_ENV) else {
            return Ok(Self {
                credentials: Vec::new(),
            });
        };
        if encoded.trim().is_empty() {
            return Ok(Self {
                credentials: Vec::new(),
            });
        }
        let configured: Vec<ModelClientCredential> =
            serde_json::from_str(&encoded).map_err(|error| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!("{MODEL_ROUTER_CLIENT_IDENTITIES_ENV} is invalid: {error}"),
                )
            })?;
        if configured.is_empty() {
            return Ok(Self {
                credentials: Vec::new(),
            });
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
                    user_id: None,
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
        // This checks a table for internal consistency. With no table there is
        // nothing inconsistent: those clients are resolved against Skarbiec,
        // where their alias set is a capability rather than a line in an
        // environment variable this process was started with.
        if self.credentials.is_empty() {
            return Ok(());
        }
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
    fn from_env(require_exact_aliases: bool) -> Result<Self, std::io::Error> {
        let encoded = match std::env::var(MODEL_ALIASES_ENV) {
            Ok(encoded) => encoded,
            Err(_) if !require_exact_aliases => "{}".to_owned(),
            Err(_) => {
                // A bare "is required" sent one supervised process into 3869
                // restarts against a cause no amount of retrying reaches: the
                // aliases are assembled by the launcher, so the binary started
                // on its own can never find them. Name the path that works.
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!(
                        "{MODEL_ALIASES_ENV} is required and is assembled by \
                         scripts/start-with-skarbiec.sh from the sealed policy directory. \
                         Starting the binary directly cannot obtain it: launch the gateway \
                         through that script, or export the variable yourself. Restarting \
                         an unlaunched process will not repair this."
                    ),
                ));
            }
        };
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
        // The named aliases are a contract with callers that ship against them,
        // so every one must be present. Equality went further and forbade any
        // additional name, which is why an operator could not add `smol` or
        // `best-vision` without editing this binary: the gateway refused to
        // start on the very alias the operator had just declared. Require the
        // named set, permit the rest.
        let required = MODEL_ALIASES
            .iter()
            .copied()
            .map(str::to_string)
            .collect::<HashSet<_>>();
        if require_exact_aliases {
            let declared = effective_routes.keys().cloned().collect::<HashSet<_>>();
            let missing = required.difference(&declared).cloned().collect::<Vec<_>>();
            if !missing.is_empty() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!(
                        "{MODEL_ALIASES_ENV} is missing required alias(es): {}",
                        missing.join(", ")
                    ),
                ));
            }
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
            let supported = alias_route_shape_supported(alias.as_str(), route);
            if !supported {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!(
                        "{MODEL_ALIASES_ENV} route for {alias} is not a shape this alias can take"
                    ),
                ));
            }
            // A missing provider capability and a malformed alias table are
            // different faults, and only one of them is this gateway's
            // configuration. The shape above is: refuse, because a wrong table
            // would route callers somewhere unintended. A credential that was
            // never issued on this host is the environment, and refusing to
            // start for it takes down every alias whose provider IS present --
            // which is how one absent subscription left the whole fleet with no
            // gateway at all. Start, serve what is serviceable, and let the
            // aliases that have no credential resolve to nothing, exactly as an
            // alias nobody declared already does.
            if alias_requires_direct_capability(alias.as_str(), route)
                && !crate::providers::adapter::provider_id_from_route(route)
                    .is_some_and(crate::gateway::broker::provider_capability_configured)
            {
                warn!(
                    event = "alias_provider_capability_absent",
                    alias = %alias,
                    route = %route,
                    "no provider capability was issued for this route; the alias will not serve"
                );
            }
        }
        Ok(Self {
            routes,
            routes_file,
        })
    }

    /// A route Brama can actually authenticate to, or nothing.
    ///
    /// Startup no longer refuses over a provider capability that was never
    /// issued here, so the check has to live where the route is used. Nothing
    /// is the honest answer: it is what an alias nobody declared already
    /// returns, and every caller of these two already handles it. Routing
    /// anyway would send a caller to a provider this gateway holds no
    /// credential for, and report it as that provider's refusal.
    fn serviceable(alias: &str, route: String) -> Option<String> {
        if !alias_requires_direct_capability(alias, &route) {
            return Some(route);
        }
        if crate::providers::adapter::provider_id_from_route(&route)
            .is_some_and(crate::gateway::broker::provider_capability_configured)
        {
            return Some(route);
        }
        warn!(
            event = "alias_route_unserviceable",
            alias = %alias,
            route = %route,
            "refusing an alias whose provider capability was never issued"
        );
        None
    }

    fn source(&self, alias: &str) -> Option<String> {
        if let Some(path) = self.routes_file.as_deref() {
            match crate::core::inference_routes::resolve(path, alias) {
                Ok(Some(route)) => return Self::serviceable(alias, route),
                Ok(None) => {}
                Err(error) => {
                    warn!(event = "inference_routes_invalid", %error);
                    return None;
                }
            }
        }
        self.routes
            .get(alias)
            .cloned()
            .and_then(|route| Self::serviceable(alias, route))
    }

    /// The chat route for one alias, plus its fallback chain.
    ///
    /// Only the two aliases that promise a different capability are refused
    /// here. This was an allowlist of five chat names, which meant an
    /// operator-defined alias passed startup validation and then served
    /// nothing: `alias_route_shape_supported` accepted it and this returned
    /// `None` for it, so the alias existed and was permanently unroutable.
    fn chat_route(&self, alias: &str) -> (Option<String>, Vec<String>) {
        if matches!(alias, WISENT_EMBEDDING_ALIAS | WISENT_MODERATION_ALIAS) {
            return (None, Vec::new());
        }
        if let Some(path) = self.routes_file.as_deref() {
            match crate::core::inference_routes::route_chain(path, alias) {
                Ok(Some(chain)) => {
                    let mut destinations = chain
                        .into_iter()
                        .filter_map(|route| Self::serviceable(alias, route));
                    return (destinations.next(), destinations.collect::<Vec<String>>());
                }
                Ok(None) => {}
                Err(error) => {
                    warn!(event = "inference_routes_invalid", %error);
                    return (None, Vec::new());
                }
            }
        }
        (
            self.routes
                .get(alias)
                .cloned()
                .and_then(|route| Self::serviceable(alias, route)),
            Vec::new(),
        )
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

/// Resolve a bearer against the configured service authority, then against
/// Wisent Identity for the account-scoped desktop surface. Both authorities
/// fail closed and the result is cached only briefly so revocation remains
/// effective without restarting the gateway.
async fn identity_from_authority(headers: &HeaderMap) -> Option<ModelClientIdentity> {
    let bearer = presented_bearer(headers)?;
    let digest = hex::encode(Sha256::digest(bearer.as_bytes()));
    if let Some(cached) = cached_identity(&digest) {
        return cached;
    }
    let resolved = match ask_authority(&bearer).await {
        Some(identity) => Some(identity),
        None => ask_wisent_identity(&bearer).await,
    };
    remember_identity(digest, resolved.clone());
    resolved
}

fn presented_bearer(headers: &HeaderMap) -> Option<String> {
    let mut values = headers.get_all(AUTHORIZATION).iter();
    let value = values.next()?;
    if values.next().is_some() {
        return None;
    }
    value
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
        .filter(|token| !token.is_empty())
        .map(str::to_string)
}

type IdentityCache =
    std::sync::Mutex<HashMap<String, (std::time::Instant, Option<ModelClientIdentity>)>>;
static IDENTITY_CACHE: std::sync::OnceLock<IdentityCache> = std::sync::OnceLock::new();

fn identity_cache_ttl() -> std::time::Duration {
    std::time::Duration::from_secs("5".parse().expect("static number"))
}

fn cached_identity(digest: &str) -> Option<Option<ModelClientIdentity>> {
    let cache = IDENTITY_CACHE.get_or_init(Default::default);
    let guard = cache.lock().ok()?;
    let (seen, identity) = guard.get(digest)?;
    if seen.elapsed() > identity_cache_ttl() {
        return None;
    }
    Some(identity.clone())
}

fn remember_identity(digest: String, identity: Option<ModelClientIdentity>) {
    let cache = IDENTITY_CACHE.get_or_init(Default::default);
    if let Ok(mut guard) = cache.lock() {
        let ttl = identity_cache_ttl();
        guard.retain(|_, (seen, _)| seen.elapsed() <= ttl);
        guard.insert(digest, (std::time::Instant::now(), identity));
    }
}

async fn ask_authority(bearer: &str) -> Option<ModelClientIdentity> {
    let base = std::env::var("WC_SKARBIEC_URL").ok()?;
    let consumer = std::env::var("BRAMA_SKARBIEC_CONSUMER").ok()?;
    let token_file = std::env::var("BRAMA_SKARBIEC_TOKEN_FILE").ok()?;
    let own = std::fs::read_to_string(&token_file).ok()?;
    let client = crate::providers::adapter::control_client().ok()?;
    let response = client
        .post(format!(
            "{}/v1/tokens/introspect",
            base.trim_end_matches('/')
        ))
        .header("X-Consumer", consumer)
        .bearer_auth(own.trim())
        .json(&json!({"token": bearer}))
        .send()
        .await
        .ok()?;
    if !response.status().is_success() {
        return None;
    }
    let answer: Value = response.json().await.ok()?;
    if answer.get("active").and_then(Value::as_bool) != Some(true) {
        return None;
    }
    let client_id = answer.get("consumer").and_then(Value::as_str)?.to_string();
    let agent_id = answer
        .get("audience")
        .and_then(Value::as_str)
        .filter(|audience| valid_agent_id(audience))
        .map(str::to_string);
    let routes: HashSet<String> = answer
        .get("capabilities")
        .and_then(Value::as_array)?
        .iter()
        .filter(|capability| {
            capability.get("action").and_then(Value::as_str) == Some("call")
                && capability.get("item").and_then(Value::as_str) == Some("brama")
        })
        .filter_map(|capability| capability.get("field").and_then(Value::as_str))
        .map(str::to_string)
        .collect();
    if routes.is_empty() {
        return None;
    }
    Some(ModelClientIdentity {
        client_id,
        agent_id,
        user_id: None,
        allowed_models: Some(routes),
    })
}
async fn ask_wisent_identity(bearer: &str) -> Option<ModelClientIdentity> {
    let base = std::env::var("BRAMA_WISENT_AUTH_URL").ok()?;
    let anon_key = std::env::var("BRAMA_WISENT_AUTH_ANON_KEY").ok()?;
    if anon_key.trim().is_empty() {
        return None;
    }
    let mut endpoint = reqwest::Url::parse(base.trim()).ok()?;
    if endpoint.scheme() != "https" {
        return None;
    }
    endpoint.set_path("/auth/v1/user");
    endpoint.set_query(None);
    endpoint.set_fragment(None);
    let response = crate::providers::adapter::control_client()
        .ok()?
        .get(endpoint)
        .header("apikey", anon_key)
        .bearer_auth(bearer)
        .send()
        .await
        .ok()?;
    if !response.status().is_success() {
        return None;
    }
    let bytes = response.bytes().await.ok()?;
    if bytes.len() > 64 * 1024 {
        return None;
    }
    let answer: Value = serde_json::from_slice(&bytes).ok()?;
    let user_id = answer.get("id").and_then(Value::as_str)?;
    let normalized = uuid::Uuid::parse_str(user_id).ok()?.to_string();
    Some(ModelClientIdentity {
        client_id: BRAMA_USER_CLIENT_ID.to_string(),
        agent_id: None,
        user_id: Some(normalized),
        allowed_models: Some(HashSet::new()),
    })
}

fn is_account_path(path: &str) -> bool {
    path == "/v1/account/subscriptions" || path.starts_with("/v1/account/subscriptions/")
}

async fn require_model_bearer(
    State(auth): State<ModelIngressAuth>,
    mut request: axum::extract::Request,
    next: Next,
) -> Response {
    let identity = match auth.identity_for(request.headers()) {
        Some(identity) => identity,
        // The table is a copy of the vault taken at boot: it cannot expire, it
        // cannot be revoked, and a client registered since this process started
        // is absent from it. Ask the authority that issued the credential
        // instead of refusing on the strength of a snapshot.
        None => match identity_from_authority(request.headers()).await {
            Some(identity) => identity,
            None => return api_error(StatusCode::UNAUTHORIZED, "unauthorized").into_response(),
        },
    };
    if identity.allowed_models.is_some()
        && !is_account_path(request.uri().path())
        && !matches!(
            request.uri().path(),
            // Every inference and discovery path a model-scoped bearer may
            // reach. The three chat formats are one workflow, so a client
            // allowed to complete a chat is allowed to complete the same chat
            // in the dialect it speaks; the model allowlist itself is enforced
            // per request, further in.
            "/v1/chat/completions"
                | "/v1/messages"
                | "/v1/responses"
                | "/v1/embeddings"
                | "/v1/moderations"
                | "/v1/models"
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

/// Peers whose hop is already encrypted before it reaches this process.
///
/// The fleet's mesh authenticates and encrypts node to node, so a request
/// arriving from one of those nodes has had exactly the protection this guard
/// exists to demand -- it simply cannot be seen from inside the request. That
/// is a narrow statement about named peers, not a general exemption for plain
/// HTTP: the list is written out by whoever renders this unit, from the hosts
/// the registry declares, and an address that is not on it is refused like any
/// other.
fn encrypted_transport_peer(peer: std::net::IpAddr) -> bool {
    std::env::var("BRAMA_ENCRYPTED_PEER_IPS")
        .ok()
        .is_some_and(|configured| {
            configured
                .split(',')
                .filter_map(|value| value.trim().parse::<std::net::IpAddr>().ok())
                .any(|trusted| trusted == peer)
        })
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

/// The address this process itself bound.
///
/// A request whose source is the address the gateway listens on came from this
/// machine and never crossed a network, so it has the protection this guard
/// demands for the same reason a loopback request does. Without this, a
/// gateway placed on a routable address cannot be exercised from the host it
/// runs on at all: `/health` answers, every other path returns 426, and an
/// operator diagnosing it concludes the gateway is broken. That is not a
/// hypothetical - it is where this comment came from.
fn own_bound_address(peer: std::net::IpAddr) -> bool {
    std::env::var("BRAMA_BIND_ADDRESS")
        .ok()
        .and_then(|configured| configured.trim().parse::<std::net::IpAddr>().ok())
        .is_some_and(|bound| bound == peer)
}

async fn require_secure_transport(request: axum::extract::Request, next: Next) -> Response {
    let forwarded = forwarded_proto_is_https(request.headers());
    let peer = request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|connect_info| connect_info.0.ip());
    let loopback = peer.is_some_and(|address| address.is_loopback() || own_bound_address(address));
    let trusted_https_proxy = peer.is_some_and(trusted_forwarded_peer) && forwarded == Some(true);
    let already_encrypted = peer.is_some_and(encrypted_transport_peer);
    if loopback || trusted_https_proxy || already_encrypted {
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
    #[serde(default)]
    tool_choice: Option<serde_json::Value>,
    #[serde(default, rename = "billingTarget")]
    billing_target: Option<BillingTarget>,
    /// Ask for server-sent events instead of one buffered completion.
    #[serde(default)]
    stream: bool,
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
    if normalized.contains("no credits remaining")
        || normalized.contains("insufficient_quota")
        || normalized.contains("exceeded your current quota")
        || normalized.contains("billing hard limit has been reached")
    {
        return ModelErrorContract {
            status: StatusCode::BAD_GATEWAY,
            error_type: "provider_error",
            code: "provider_quota_exhausted",
            retryable: false,
        };
    }
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
    // A refused redemption is not a busy provider. Skarbiec says "capability is
    // not issued, has expired, has no uses left, or its authorization id does
    // not match", and every one of those is a broken authorization chain that
    // no amount of waiting repairs. Reporting it as capacity told the caller to
    // retry and told the operator to look for a subscription, which is where a
    // whole day went before the log was read directly.
    if normalized.contains("redemption denied")
        || normalized.contains("authorization id does not match")
        || normalized.contains("capability is not issued")
        || normalized.contains("no uses left")
        || normalized.contains("capability redemption denied")
    {
        return ModelErrorContract {
            status: StatusCode::SERVICE_UNAVAILABLE,
            error_type: "authorization_error",
            code: "credential_unauthorized",
            retryable: false,
        };
    }
    // A pool that produced no credential at all is the same shape of failure one
    // layer earlier: no provider was asked, so there is no capacity to wait for.
    // The chain that would have produced the secret -- capability, read grant,
    // installation trust material -- is broken, and only an operator repairs it.
    if normalized.contains("could be redeemed for agent") {
        return ModelErrorContract {
            status: StatusCode::SERVICE_UNAVAILABLE,
            error_type: "authorization_error",
            code: "credential_unauthorized",
            retryable: false,
        };
    }
    // A pool the provider rejected is not a pool that is busy. Waiting cannot
    // reach it: somebody has to authorize the subscription again, and saying
    // `429 capacity_error, retryable: true` sent this workstation's agent into
    // retries for hours against a credential the provider had already burnt.
    if normalized.contains("re-authorization required") {
        return ModelErrorContract {
            status: StatusCode::SERVICE_UNAVAILABLE,
            error_type: "authorization_error",
            code: "subscription_reauthorization_required",
            retryable: false,
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
        // Asking to stream a route this gateway can only buffer is a request
        // this gateway cannot serve, not a provider that failed: nothing
        // upstream was contacted, and `502 provider_failure` sent the caller
        // looking at a provider that never saw the request.
        || normalized.contains("streaming is supported for")
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

/// The same failure in the fleet's envelope, for the operator reading the log.
///
/// The contract above is what clients read and nothing here touches it. The
/// envelope is additional and it stays in the log: a new key in the HTTP error
/// body is a wire change, and a migration whose whole promise is "no behaviour
/// change" does not get to make one. Where the two disagree -- the contract is
/// coarser at a handful of provider statuses -- both are on the line, so an
/// operator can see that they do.
fn model_error_envelope(message: &str, contract: ModelErrorContract) -> String {
    failure::envelope(
        POINT_MODEL_REQUEST,
        failure::code_for_message(message, contract.code),
        IMPACT_MODEL_REQUEST,
        message,
    )
    .to_json()
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
) -> Response {
    let req: ChatCompletionRequest = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(error) => {
            return api_error(StatusCode::BAD_REQUEST, &format!("invalid JSON: {error}"))
                .into_response();
        }
    };
    if req.messages.is_empty() {
        return api_error(StatusCode::BAD_REQUEST, "messages must not be empty").into_response();
    }
    if req.max_tokens == u32::default() || req.max_tokens > max_output_tokens() {
        return api_error(
            StatusCode::BAD_REQUEST,
            &format!("max_tokens must be between one and {}", max_output_tokens()),
        )
        .into_response();
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
        )
        .into_response();
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
    let requested_model = req.model.as_deref().unwrap_or("").trim().to_string();
    if requested_model.is_empty() {
        return api_error(StatusCode::BAD_REQUEST, "missing field `model`").into_response();
    }
    let (system, non_system_messages) = split_system_message(messages);
    let request = ModelRequest {
        messages: non_system_messages,
        model: String::new(),
        max_tokens: req.max_tokens,
        temperature: req.temperature,
        system,
        tools: req.tools,
        tool_choice: req.tool_choice,
        billing_target: req.billing_target,
    };
    let (dispatched, meta) = match route_model_call(
        &client_identity,
        &aliases,
        &headers,
        &body,
        &requested_model,
        request,
        req.stream,
    )
    .await
    {
        Ok(routed) => routed,
        Err(response) => return response,
    };
    match dispatched {
        DispatchedCall::Buffered(mut resp) => {
            tally_and_log_buffered(&mut resp, &client_identity, &meta, &requested_model);
            if !resp.success {
                return failure_response(&mut resp);
            }
            // Alias telemetry stays attached to the stable logical selector.
            crate::core::perf::record(&requested_model, resp.latency_ms, resp.output_tokens);
            let has_tool_calls = resp.tool_calls.as_ref().is_some_and(|tc| !tc.is_empty());
            let finish_reason = if has_tool_calls { "tool_calls" } else { "stop" };
            let response_model = meta.reported_model(&requested_model, &resp.model);
            let body = serde_json::to_value(ChatCompletionResponse {
                id: meta.request_id.clone(),
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
            (StatusCode::OK, Json(body)).into_response()
        }
        DispatchedCall::Committed(routed) => {
            log_stream_commit(&routed, &client_identity, &meta, &requested_model);
            let stream = ChatChunkStream {
                rx: routed.events,
                pending: std::collections::VecDeque::new(),
                request_id: meta.request_id.clone(),
                model: meta.reported_model(&requested_model, &routed.model),
                requested_model,
                created: epoch_seconds(),
                started: meta.started,
                terminated: false,
                accounted: false,
                sent_role: false,
                saw_tool_calls: false,
                finish_reason: None,
                usage: None,
                input_tokens: 0,
                output_tokens: 0,
                failed: false,
            };
            sse_response(stream)
        }
    }
}

/// One Anthropic Messages request, routed exactly as its chat-completions
/// sibling is and answered in the format the caller speaks.
///
/// Nothing about routing, identity, or billing changes with the wire format:
/// the model must still be a canonical route or a supported selector, a
/// subscription still needs the caller's own signed identity, and the same
/// bounded attempts apply. Only the request and response shapes differ, and
/// they are translated in [`crate::core::wire`].
async fn anthropic_messages(
    Extension(client_identity): Extension<ModelClientIdentity>,
    Extension(aliases): Extension<ModelAliases>,
    headers: axum::http::HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let call = match crate::core::wire::anthropic_request(&body) {
        Ok(call) => call,
        Err(message) => return api_error(StatusCode::BAD_REQUEST, &message).into_response(),
    };
    let requested_model = call.model;
    let (dispatched, meta) = match route_model_call(
        &client_identity,
        &aliases,
        &headers,
        &body,
        &requested_model,
        call.request,
        call.stream,
    )
    .await
    {
        Ok(routed) => routed,
        Err(response) => return response,
    };
    let message_id = format!("msg_{}", uuid::Uuid::new_v4().simple());
    match dispatched {
        DispatchedCall::Buffered(mut resp) => {
            tally_and_log_buffered(&mut resp, &client_identity, &meta, &requested_model);
            if !resp.success {
                return failure_response(&mut resp);
            }
            crate::core::perf::record(&requested_model, resp.latency_ms, resp.output_tokens);
            let model = meta.reported_model(&requested_model, &resp.model);
            (
                StatusCode::OK,
                Json(crate::core::wire::anthropic_response(
                    &message_id,
                    &model,
                    &resp,
                )),
            )
                .into_response()
        }
        DispatchedCall::Committed(routed) => {
            log_stream_commit(&routed, &client_identity, &meta, &requested_model);
            let model = meta.reported_model(&requested_model, &routed.model);
            sse_response(crate::core::wire::AnthropicEventStream::new(
                routed.events,
                message_id,
                model,
                stream_accounting(requested_model, meta.started),
            ))
        }
    }
}

/// One OpenAI Responses request, on the same routing decision as every other
/// format this gateway serves.
async fn openai_responses(
    Extension(client_identity): Extension<ModelClientIdentity>,
    Extension(aliases): Extension<ModelAliases>,
    headers: axum::http::HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let call = match crate::core::wire::responses_request(&body) {
        Ok(call) => call,
        Err(message) => return api_error(StatusCode::BAD_REQUEST, &message).into_response(),
    };
    let requested_model = call.model;
    let (dispatched, meta) = match route_model_call(
        &client_identity,
        &aliases,
        &headers,
        &body,
        &requested_model,
        call.request,
        call.stream,
    )
    .await
    {
        Ok(routed) => routed,
        Err(response) => return response,
    };
    let response_id = format!("resp_{}", uuid::Uuid::new_v4().simple());
    match dispatched {
        DispatchedCall::Buffered(mut resp) => {
            tally_and_log_buffered(&mut resp, &client_identity, &meta, &requested_model);
            if !resp.success {
                return failure_response(&mut resp);
            }
            crate::core::perf::record(&requested_model, resp.latency_ms, resp.output_tokens);
            let model = meta.reported_model(&requested_model, &resp.model);
            (
                StatusCode::OK,
                Json(crate::core::wire::responses_response(
                    &response_id,
                    &model,
                    epoch_seconds(),
                    &resp,
                )),
            )
                .into_response()
        }
        DispatchedCall::Committed(routed) => {
            log_stream_commit(&routed, &client_identity, &meta, &requested_model);
            let model = meta.reported_model(&requested_model, &routed.model);
            sse_response(crate::core::wire::ResponsesEventStream::new(
                routed.events,
                response_id,
                model,
                stream_accounting(requested_model, meta.started),
            ))
        }
    }
}

/// Whether this call was answered in one piece or committed as a stream.
enum DispatchedCall {
    Buffered(ModelResponse),
    Committed(RoutedStream),
}

/// What the routing decision produced besides the call itself, so every
/// format shapes its answer from the same facts.
struct RoutedCallMeta {
    routing_mode: &'static str,
    alias_source: Option<String>,
    request_id: String,
    selected_model: String,
    started: Instant,
}

impl RoutedCallMeta {
    /// The model name the caller should be told about: the alias it asked for
    /// when one resolved, and the canonical route it actually reached
    /// otherwise. An alias exists so a caller does not have to know which
    /// route is behind it today.
    fn reported_model(&self, requested_model: &str, resolved: &str) -> String {
        if self.alias_source.is_some() {
            requested_model.to_string()
        } else {
            resolved.to_string()
        }
    }
}

/// Split the OpenAI system-role message out of a conversation.
///
/// The stateless provider adapters take the system prompt as its own field, so
/// the first system-role message becomes that field and the rest of the
/// conversation travels unchanged.
fn split_system_message(messages: Vec<Message>) -> (Option<String>, Vec<Message>) {
    let mut system: Option<String> = None;
    let mut rest: Vec<Message> = Vec::with_capacity(messages.len());
    for message in messages {
        if message.role == "system" && system.is_none() {
            system = message.content.as_str().map(|value| value.to_string());
            continue;
        }
        rest.push(message);
    }
    (system, rest)
}

fn epoch_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or_default()
}

/// Wrap one encoder in the SSE response every streaming format shares.
///
/// The keep-alive comment is what stops an idle proxy from closing a stream
/// that is legitimately silent while the model thinks; it is a comment frame,
/// so no client parses it as content.
fn sse_response<S>(stream: S) -> Response
where
    S: futures_core::Stream<Item = Result<axum::response::sse::Event, std::convert::Infallible>>
        + Send
        + 'static,
{
    axum::response::sse::Sse::new(stream)
        .keep_alive(
            axum::response::sse::KeepAlive::new()
                .interval(Duration::from_secs(15))
                .text("keep-alive"),
        )
        .into_response()
}

/// The closure a wire encoder reports its stream's end through.
///
/// Statistics belong to this layer; the encoders in [`crate::core::wire`] know
/// the numbers but not where they are kept, so they are handed this instead of
/// a static they would have to reach into.
fn stream_accounting(
    requested_model: String,
    started: Instant,
) -> crate::core::wire::StreamAccounting {
    Box::new(move |input_tokens, output_tokens, failed| {
        TOTAL_INPUT_TOKENS.fetch_add(u64::from(input_tokens), Ordering::Relaxed);
        TOTAL_OUTPUT_TOKENS.fetch_add(u64::from(output_tokens), Ordering::Relaxed);
        if failed {
            TOTAL_FAILURES.fetch_add(u64::from(true), Ordering::Relaxed);
        } else {
            crate::core::perf::record(
                &requested_model,
                started.elapsed().as_secs_f64() * 1_000.0,
                output_tokens,
            );
        }
    })
}

/// Resolve one model call and execute it, buffered or streamed.
///
/// This is the whole routing decision, shared by every wire format: selector
/// detection, the client's model allowlist, alias resolution, whether the call
/// is caller-scoped -- and therefore billed to the caller's own
/// subscription -- and the bounded dispatch that follows. `Err` is a refusal
/// that happened before any provider was contacted; a provider failure comes
/// back as [`DispatchedCall::Buffered`] carrying it, because that is the same
/// shape the buffered path has always reported.
async fn route_model_call(
    client_identity: &ModelClientIdentity,
    aliases: &ModelAliases,
    headers: &HeaderMap,
    raw_body: &axum::body::Bytes,
    requested_model: &str,
    mut request: ModelRequest,
    stream: bool,
) -> Result<(DispatchedCall, RoutedCallMeta), Response> {
    let any_subscription = is_any_subscription_selector(requested_model);
    let any_vision_capable_subscription =
        is_any_vision_capable_subscription_selector(requested_model);
    let account_agent = account_agent_for_route(client_identity, requested_model).await;
    if !client_identity.authorizes_model(requested_model) && account_agent.is_none() {
        return Err(api_error(StatusCode::FORBIDDEN, "forbidden").into_response());
    }
    let task_subscription = task_subscription_selector(requested_model);
    let (alias_source, alias_fallbacks) = aliases.chat_route(requested_model);
    // `best` is a selector, not a route. The alias resolves to one configured
    // provider route and that route is the caller's first choice, but the name
    // means "the best subscription model this caller can be served from", so
    // the rest of the caller's ranked routes stand behind it. Dispatching the
    // configured route alone is what answered `503` naming codex, with
    // `attempts: 0`, in a second when the same agent's kimi subscription was
    // serving: one refused redemption and the request was over.
    let best_subscription = requested_model == BEST_ALIAS
        || alias_source
            .as_deref()
            .is_some_and(|route| route == BEST_ALIAS);
    let selected_model = alias_source
        .as_deref()
        .unwrap_or(requested_model)
        .to_string();
    // A route that delegates to `best` again names no provider, so it is a
    // preference nothing can be preferred to.
    let preferred_route = Some(selected_model.as_str())
        .filter(|route| is_subscription_model(route))
        .map(str::to_owned);
    let canonical_model = is_subscription_model(&selected_model);
    if !(any_subscription
        || any_vision_capable_subscription
        || best_subscription
        || task_subscription.is_some()
        || canonical_model)
    {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "model must be a canonical provider/model route or a supported selector",
        )
        .into_response());
    }
    // Subscription dispatch is for credentials that belong to the caller, and
    // the four tests below say when that is the case: an explicit subscription
    // selector, a named billing target, or a provider whose usage must bill to
    // the caller's own subscription -- `claude-code`, `codex`, `kimi`.
    //
    // Presenting agent auth used to be a fifth. It protected nothing those four
    // do not already cover, and it inverted the access rule: the same client,
    // with the same bearer and the same alias, was served from the deployment's
    // own provider when it stayed anonymous and refused with "no active
    // credential for agent" the moment it proved who it was. The catalogue said
    // the opposite in the same breath, marking those models available to agent
    // callers. Proving more identity cannot grant less access. The dispatch
    // branches below must therefore use this predicate too; a bearer-bound
    // `agent_id` remains audit identity, not an entitlement by itself.
    let caller_scoped_request = account_agent.is_some()
        || any_subscription
        || any_vision_capable_subscription
        || best_subscription
        || task_subscription.is_some()
        || request.billing_target.is_some()
        || provider_requires_caller_identity(&selected_model);
    let routing_mode = if task_subscription.is_some() {
        "task"
    } else if any_vision_capable_subscription {
        "any-vision-capable"
    } else if any_subscription {
        "any"
    } else if best_subscription {
        "best"
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
        agent_id = client_identity
            .agent_id
            .as_deref()
            .or(account_agent.as_deref())
            .unwrap_or("none"),
        routing_mode,
        requested_model,
        selected_model = %selected_model,
        streamed = stream,
        "routing request accepted"
    );
    let started = Instant::now();
    // The deadline bounds route selection, rotation, and the provider's
    // response headers. For a stream it stops there by design: once events
    // flow, "how long may this take" is the provider's idle timeout, not a
    // budget that would cut a generation mid-sentence.
    let deadline = request_deadline().saturating_mul(
        u32::try_from(alias_fallbacks.len().saturating_add(usize::from(true))).unwrap_or(u32::MAX),
    );
    request.model = selected_model.clone();
    let meta = RoutedCallMeta {
        routing_mode,
        alias_source,
        request_id,
        selected_model,
        started,
    };
    let dispatched = tokio::time::timeout(deadline, async {
        if stream {
            let opened = if let Some(task) = task_subscription.as_deref() {
                dispatch_task_subscription_stream(headers, &request, raw_body, task).await
            } else if any_vision_capable_subscription {
                dispatch_any_vision_capable_subscription_stream(headers, &request, raw_body).await
            } else if any_subscription {
                dispatch_any_subscription_stream(headers, &request, raw_body).await
            } else if best_subscription {
                if let Some(agent_id) = client_identity.agent_id.as_deref() {
                    dispatch_best_subscription_stream_for_agent(
                        agent_id,
                        &request,
                        preferred_route.as_deref(),
                    )
                    .await
                } else {
                    dispatch_best_subscription_stream(
                        headers,
                        &request,
                        raw_body,
                        preferred_route.as_deref(),
                    )
                    .await
                }
            } else if let Some(account_agent) = account_agent.as_deref() {
                dispatch_subscription_stream_for_agent(account_agent, &request).await
            } else if caller_scoped_request {
                dispatch_subscription_stream(headers, &request, raw_body).await
            } else {
                dispatch_direct_with_fallback_stream(&request, &alias_fallbacks).await
            };
            match opened {
                Ok(routed) => DispatchedCall::Committed(routed),
                Err(failure) => DispatchedCall::Buffered(failure),
            }
        } else if let Some(task) = task_subscription.as_deref() {
            DispatchedCall::Buffered(
                dispatch_task_subscription(headers, &request, raw_body, task).await,
            )
        } else if any_vision_capable_subscription {
            DispatchedCall::Buffered(
                dispatch_any_vision_capable_subscription(headers, &request, raw_body).await,
            )
        } else if any_subscription {
            DispatchedCall::Buffered(dispatch_any_subscription(headers, &request, raw_body).await)
        } else if best_subscription {
            DispatchedCall::Buffered(
                if let Some(agent_id) = client_identity.agent_id.as_deref() {
                    dispatch_best_subscription_for_agent(
                        agent_id,
                        &request,
                        preferred_route.as_deref(),
                    )
                    .await
                } else {
                    dispatch_best_subscription(
                        headers,
                        &request,
                        raw_body,
                        preferred_route.as_deref(),
                    )
                    .await
                },
            )
        } else if let Some(account_agent) = account_agent.as_deref() {
            DispatchedCall::Buffered(dispatch_subscription_for_agent(account_agent, &request).await)
        } else if caller_scoped_request {
            DispatchedCall::Buffered(dispatch_subscription(headers, &request, raw_body).await)
        } else {
            DispatchedCall::Buffered(
                dispatch_direct_with_fallback(&request, &alias_fallbacks).await,
            )
        }
    })
    .await
    .unwrap_or_else(|_| {
        DispatchedCall::Buffered(ModelResponse::failure(
            &meta.selected_model,
            "whole request deadline exceeded".into(),
        ))
    });
    Ok((dispatched, meta))
}

/// Fold one buffered answer into the process statistics and the request log.
fn tally_and_log_buffered(
    resp: &mut ModelResponse,
    client_identity: &ModelClientIdentity,
    meta: &RoutedCallMeta,
    requested_model: &str,
) {
    if resp.latency_ms == f64::default() {
        resp.latency_ms = meta.started.elapsed().as_millis() as f64;
    }
    TOTAL_REQUESTS.fetch_add(1, Ordering::Relaxed);
    TOTAL_INPUT_TOKENS.fetch_add(resp.input_tokens as u64, Ordering::Relaxed);
    TOTAL_OUTPUT_TOKENS.fetch_add(resp.output_tokens as u64, Ordering::Relaxed);
    TOTAL_PROVIDER_ATTEMPTS.fetch_add(resp.attempts as u64, Ordering::Relaxed);
    if !resp.success {
        TOTAL_FAILURES.fetch_add(u64::from(true), Ordering::Relaxed);
    }
    let failure_contract = resp.error.as_deref().map(model_error_contract);
    // `error_code` below is Brama's own contract code, unchanged, because log
    // pipelines read it. `envelope` is the fleet's reading of the same failure.
    let failure_envelope = resp
        .error
        .as_deref()
        .zip(failure_contract)
        .map(|(message, contract)| model_error_envelope(message, contract))
        .unwrap_or_else(|| "none".to_owned());
    info!(
        event = "routing_complete",
        request_id = %meta.request_id,
        client_id = %client_identity.client_id,
        routing_mode = meta.routing_mode,
        requested_model,
        selected_model = %resp.model,
        attempts = resp.attempts,
        success = resp.success,
        streamed = false,
        elapsed_ms = meta.started.elapsed().as_millis() as u64,
        error_code = failure_contract.map(|contract| contract.code).unwrap_or("none"),
        retryable = failure_contract.is_some_and(|contract| contract.retryable),
        input_tokens = resp.input_tokens,
        output_tokens = resp.output_tokens,
        operator_action_required = failure_contract.is_some_and(|contract| !contract.retryable),
        envelope = %failure_envelope,
        "routing request completed"
    );
}

/// Shape one refused call as this gateway's normalized error document.
///
/// Every format shares it: a caller that cannot get its generation needs the
/// contract code and retryability, and those are Brama's, not the wire's.
fn failure_response(resp: &mut ModelResponse) -> Response {
    let message = resp.error.take().unwrap_or_default();
    let contract = model_error_contract(&message);
    error_response(
        contract.status,
        contract.error_type,
        contract.code,
        &message,
        contract.retryable,
        resp.attempts,
    )
    .into_response()
}

/// Record that a stream committed: statistics that are already final, and the
/// one log line that says a generation is on its way.
fn log_stream_commit(
    routed: &RoutedStream,
    client_identity: &ModelClientIdentity,
    meta: &RoutedCallMeta,
    requested_model: &str,
) {
    TOTAL_REQUESTS.fetch_add(u64::from(true), Ordering::Relaxed);
    TOTAL_PROVIDER_ATTEMPTS.fetch_add(u64::from(routed.attempts), Ordering::Relaxed);
    info!(
        event = "routing_complete",
        request_id = %meta.request_id,
        client_id = %client_identity.client_id,
        routing_mode = meta.routing_mode,
        requested_model,
        selected_model = %routed.model,
        attempts = routed.attempts,
        success = true,
        streamed = true,
        elapsed_ms = meta.started.elapsed().as_millis() as u64,
        error_code = "none",
        "streaming request committed"
    );
}

/// Map a provider stop reason onto the OpenAI finish vocabulary.
fn openai_finish_reason(reason: Option<&str>, saw_tool_calls: bool) -> String {
    match reason {
        Some("end_turn") | Some("stop_sequence") | Some("stop") => "stop",
        Some("max_tokens") => "length",
        Some("tool_use") | Some("tool_calls") => "tool_calls",
        Some("length") | Some("content_filter") => reason.unwrap_or("stop"),
        Some(other) if other.starts_with("incomplete") => {
            if other.contains("max_output_tokens") {
                "length"
            } else {
                "stop"
            }
        }
        _ => {
            if saw_tool_calls {
                "tool_calls"
            } else {
                "stop"
            }
        }
    }
    .to_string()
}

/// One caller-facing SSE stream built from neutral provider events.
///
/// Process statistics are accounted here, at the stream's end, because that
/// is when the numbers exist; the subscription ledger is accounted one layer
/// down, where the credential that paid is known.
struct ChatChunkStream {
    rx: tokio::sync::mpsc::Receiver<StreamItem>,
    pending: std::collections::VecDeque<axum::response::sse::Event>,
    request_id: String,
    model: String,
    requested_model: String,
    created: u64,
    started: Instant,
    terminated: bool,
    accounted: bool,
    sent_role: bool,
    saw_tool_calls: bool,
    finish_reason: Option<String>,
    usage: Option<(u32, u32)>,
    input_tokens: u32,
    output_tokens: u32,
    failed: bool,
}

impl ChatChunkStream {
    fn chunk(&self, delta: Value, finish_reason: Option<&str>) -> axum::response::sse::Event {
        let mut choice = json!({ "index": 0, "delta": delta });
        match finish_reason {
            Some(reason) => choice["finish_reason"] = json!(reason),
            None => choice["finish_reason"] = Value::Null,
        }
        axum::response::sse::Event::default().data(
            serde_json::to_string(&json!({
                "id": self.request_id,
                "object": "chat.completion.chunk",
                "created": self.created,
                "model": self.model,
                "choices": [choice],
            }))
            .unwrap_or_default(),
        )
    }

    fn role_chunk(&self) -> axum::response::sse::Event {
        self.chunk(json!({ "role": "assistant" }), None)
    }

    fn usage_chunk(&self, input_tokens: u32, output_tokens: u32) -> axum::response::sse::Event {
        axum::response::sse::Event::default().data(
            serde_json::to_string(&json!({
                "id": self.request_id,
                "object": "chat.completion.chunk",
                "created": self.created,
                "model": self.model,
                "choices": [],
                "usage": {
                    "prompt_tokens": input_tokens,
                    "completion_tokens": output_tokens,
                    "total_tokens": input_tokens + output_tokens,
                },
            }))
            .unwrap_or_default(),
        )
    }

    /// Fold this stream's end into the process statistics, exactly once.
    fn account(&mut self) {
        if self.accounted {
            return;
        }
        self.accounted = true;
        TOTAL_INPUT_TOKENS.fetch_add(u64::from(self.input_tokens), Ordering::Relaxed);
        TOTAL_OUTPUT_TOKENS.fetch_add(u64::from(self.output_tokens), Ordering::Relaxed);
        if self.failed {
            TOTAL_FAILURES.fetch_add(u64::from(true), Ordering::Relaxed);
        } else {
            crate::core::perf::record(
                &self.requested_model,
                self.started.elapsed().as_secs_f64() * 1_000.0,
                self.output_tokens,
            );
        }
    }
}

impl futures_core::Stream for ChatChunkStream {
    type Item = Result<axum::response::sse::Event, std::convert::Infallible>;

    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        use std::task::Poll;
        loop {
            if let Some(event) = self.pending.pop_front() {
                return Poll::Ready(Some(Ok(event)));
            }
            if self.terminated {
                self.account();
                return Poll::Ready(None);
            }
            let item = match self.rx.poll_recv(cx) {
                Poll::Pending => return Poll::Pending,
                // The recorder closed the channel without a terminal item: an
                // abnormal end, signalled the same way as a mid-stream failure
                // -- the stream simply stops, without `data: [DONE]`.
                Poll::Ready(None) => {
                    self.terminated = true;
                    self.failed = true;
                    continue;
                }
                Poll::Ready(Some(item)) => item,
            };
            match item {
                StreamItem::Delta(StreamDelta::Text(text)) => {
                    // Queued rather than returned: the role chunk may be
                    // waiting in front of it, and a caller that receives
                    // content before the role it belongs to is reading a
                    // different conversation than the one being sent.
                    if !self.sent_role {
                        self.sent_role = true;
                        let role = self.role_chunk();
                        self.pending.push_back(role);
                    }
                    let content = self.chunk(json!({ "content": text }), None);
                    self.pending.push_back(content);
                }
                StreamItem::Delta(StreamDelta::ToolCallStart { index, id, name }) => {
                    self.saw_tool_calls = true;
                    if !self.sent_role {
                        self.sent_role = true;
                        let role = self.role_chunk();
                        self.pending.push_back(role);
                    }
                    let delta = json!({
                        "tool_calls": [{
                            "index": index,
                            "id": id,
                            "type": "function",
                            "function": { "name": name, "arguments": "" },
                        }],
                    });
                    let chunk = self.chunk(delta, None);
                    self.pending.push_back(chunk);
                }
                StreamItem::Delta(StreamDelta::ToolCallArguments { index, delta }) => {
                    let delta = json!({
                        "tool_calls": [{
                            "index": index,
                            "function": { "arguments": delta },
                        }],
                    });
                    let chunk = self.chunk(delta, None);
                    self.pending.push_back(chunk);
                }
                StreamItem::Delta(StreamDelta::Finish { reason }) => {
                    self.finish_reason =
                        Some(openai_finish_reason(reason.as_deref(), self.saw_tool_calls));
                }
                StreamItem::Delta(StreamDelta::Usage {
                    input_tokens,
                    output_tokens,
                }) => {
                    self.usage = Some((input_tokens, output_tokens));
                    self.input_tokens = input_tokens;
                    self.output_tokens = output_tokens;
                }
                StreamItem::Done => {
                    let reason = self
                        .finish_reason
                        .clone()
                        .unwrap_or_else(|| openai_finish_reason(None, self.saw_tool_calls));
                    let finish = self.chunk(json!({}), Some(&reason));
                    self.pending.push_back(finish);
                    if let Some((input_tokens, output_tokens)) = self.usage {
                        let usage = self.usage_chunk(input_tokens, output_tokens);
                        self.pending.push_back(usage);
                    }
                    self.pending
                        .push_back(axum::response::sse::Event::default().data("[DONE]"));
                    self.terminated = true;
                }
                StreamItem::Failed(message) => {
                    warn!(
                        event = "stream_failed_mid_flight",
                        request_id = %self.request_id,
                        model = %self.model,
                        error = %message,
                        "provider stream failed after commit; ending without [DONE]"
                    );
                    self.terminated = true;
                    self.failed = true;
                }
            }
        }
    }
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

#[derive(Clone)]
struct ReadinessReport {
    status: StatusCode,
    body: Value,
}

impl ReadinessReport {
    fn pending() -> Self {
        Self {
            status: StatusCode::SERVICE_UNAVAILABLE,
            body: json!({
                "ready": false,
                "reason": "readiness check has not completed",
                "providers": [],
                "denied": [],
                "routing": [],
                "unroutable": [],
                "subscriptions": [],
                "unredeemable": [],
                "unroutable_accounts": [],
                "operator_action_required": false,
                "build": crate::build_info::current(),
            }),
        }
    }
}

static READINESS_REPORT: LazyLock<tokio::sync::RwLock<ReadinessReport>> =
    LazyLock::new(|| tokio::sync::RwLock::new(ReadinessReport::pending()));

/// Return the last completed credential and routing check.
///
/// The check itself crosses the Skarbiec broker and provider discovery APIs.
/// Doing that work inside this request made each three-second deployment probe
/// cancel before a response, then start the same work again. A background loop
/// owns the expensive check; this endpoint remains a bounded status read.
async fn readyz() -> impl IntoResponse {
    let report = READINESS_REPORT.read().await.clone();
    (report.status, Json(report.body))
}

fn spawn_readiness_probe() {
    tokio::spawn(async {
        loop {
            let report = calculate_readiness().await;
            *READINESS_REPORT.write().await = report;
            tokio::time::sleep(Duration::from_secs(30)).await;
        }
    });
}

/// Does the credential chain actually work right now?
///
/// `/health` answers whether the process is up. This check redeems one
/// capability per configured provider, discovers every active subscription,
/// and performs one subscription redemption through the request path. Provider,
/// agent and subscription checks are independent, so each group runs
/// concurrently; one slow provider no longer delays every check behind it.
async fn calculate_readiness() -> ReadinessReport {
    let providers: Vec<String> = {
        let mut names: Vec<String> = crate::gateway::broker::configured_provider_capabilities()
            .into_iter()
            .collect();
        names.sort();
        names
    };

    let provider_results = join_all(providers.iter().map(|provider| async {
        let obtained = crate::gateway::broker::provider_credential(provider)
            .await
            .is_some();
        (provider.clone(), obtained)
    }))
    .await;
    let mut checked = Vec::with_capacity(provider_results.len());
    let mut denied = Vec::new();
    for (provider, obtained) in provider_results {
        if !obtained {
            denied.push(provider.clone());
        }
        checked.push(json!({ "provider": provider, "credential": obtained }));
    }

    // Obtaining a credential is only the first half. A subscription whose model
    // discovery yields nothing is active, its credential redeems, and it still
    // cannot be routed to. Every active subscription is collected once,
    // whichever agents can see it.
    let standalone = crate::gateway::broker::local_provider_credentials_enabled();
    let agents = if standalone {
        Vec::new()
    } else {
        crate::gateway::broker::configured_request_sign_agents()
    };
    let agent_results = join_all(agents.into_iter().map(|agent| async move {
        let subscriptions = crate::gateway::broker::list_subscriptions(&agent).await;
        let models =
            crate::subscription_dispatch::dispatch::registry_models_for_agent(&agent).await;
        (agent, subscriptions, models)
    }))
    .await;

    let mut routable = Vec::new();
    let mut unroutable = Vec::new();
    let mut active = std::collections::BTreeMap::<String, String>::new();
    for (agent, entries, models) in agent_results {
        let mut subscribed = Vec::new();
        for entry in entries {
            if entry.status != "active" {
                continue;
            }
            let provider = entry.provider.trim().to_string();
            subscribed.push(provider.clone());
            active.entry(entry.id).or_insert(provider);
        }
        match models {
            Ok(models) => {
                let mut per_provider = std::collections::BTreeMap::<String, usize>::new();
                for model in &models {
                    let provider =
                        crate::subscription_dispatch::dispatch::provider_for(&model.route_id)
                            .unwrap_or("unattributed")
                            .to_string();
                    *per_provider.entry(provider).or_default() += usize::from(true);
                }
                for provider in &subscribed {
                    if !per_provider.contains_key(provider) {
                        unroutable.push(json!({
                            "agent": agent,
                            "provider": provider,
                            "reason": "active subscription, no model discovered",
                        }));
                    }
                }
                routable.push(json!({
                    "agent": agent,
                    "models": models.len(),
                    "by_provider": per_provider,
                }));
            }
            Err(error) => {
                unroutable.push(json!({
                    "agent": agent,
                    "provider": "all",
                    "reason": error,
                }));
            }
        }
    }

    // The act itself: one redemption per active subscription, at the boundary
    // where a model request redeems it.
    let subscription_results = join_all(active.iter().map(|(subscription, provider)| async {
        let refusal = crate::subscription_dispatch::dispatch::probe_subscription_redemption(
            subscription,
            provider,
        )
        .await
        .err();
        (subscription.clone(), provider.clone(), refusal)
    }))
    .await;
    let mut subscriptions = Vec::with_capacity(subscription_results.len());
    let mut unredeemable = Vec::new();
    for (subscription, provider, refusal) in subscription_results {
        if refusal.is_some() {
            unredeemable.push(subscription.clone());
        }
        subscriptions.push(json!({
            "id": subscription,
            "provider": provider,
            "redeemable": refusal.is_none(),
            "reason": refusal.unwrap_or_else(|| "the credential redeemed".to_string()),
        }));
    }

    // A subscription item that loses every `brama:agent:` tag disappears from
    // normal discovery. The broker reports those explicitly without treating
    // unrelated vault entries as subscription accounts.
    let untagged: Vec<Value> = if standalone {
        Vec::new()
    } else {
        crate::gateway::broker::list_unroutable_accounts()
            .await
            .into_iter()
            .map(|account| {
                let (provider_str, reason) = match (&account.provider, &account.id) {
                    (Some(provider), Some(_id)) => {
                        let refusal = crate::subscription_dispatch::dispatch::no_active_credential_summary(provider);
                        (provider.clone(), format!(
                            "the vault holds this account and its item carries no 'brama:agent:' tag, \
                             so subscription discovery cannot see it and no agent can route to it; \
                             every request for this provider answers '{refusal}'"
                        ))
                    }
                    (Some(provider), None) => {
                        let refusal = crate::subscription_dispatch::dispatch::no_active_credential_summary(provider);
                        (provider.clone(), format!(
                            "the vault holds this account; its item carries 'brama:provider:' but no 'brama:id:' tag; \
                             subscription discovery cannot route to it without both tags; \
                             every request for this provider would answer '{refusal}'"
                        ))
                    }
                    (None, Some(_id)) => {
                        ("unknown".to_string(),
                         "the vault holds this account; its item carries 'brama:id:' but no 'brama:provider:' tag; \
                          subscription discovery cannot route to it without both tags; \
                          operator: add the missing 'brama:provider:' tag or remove this item".to_string())
                    }
                    (None, None) => {
                        ("unknown".to_string(),
                         "the vault holds this subscription account, but its item carries no 'brama:id:' or 'brama:provider:' tags; \
                          subscription discovery cannot route to it; \
                          operator: restore both tags or remove this item from the vault".to_string())
                    }
                };
                json!({
                    "id": account.id,
                    "provider": &provider_str,
                    "item": account.item,
                    "routable": false,
                    "reason": reason,
                })
            })
            .collect()
    };

    let ready = !providers.is_empty()
        && denied.is_empty()
        && unredeemable.is_empty()
        && untagged.is_empty()
        && unroutable.is_empty();
    let status = if ready {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    let reason = if providers.is_empty() {
        "no provider capability is configured"
    } else if !denied.is_empty() {
        "a configured provider credential could not be obtained; the authorization chain is broken, not busy"
    } else if !unredeemable.is_empty() {
        "an active subscription's credential did not redeem just now, so the next request that needs it will be refused whatever the catalogue lists"
    } else if !untagged.is_empty() {
        "the vault holds a subscription account whose item carries no agent tag: it is unroutable, and no subscription listing can report it"
    } else if !unroutable.is_empty() {
        "every credential was obtained, and at least one active subscription contributes no model: it cannot be routed to"
    } else {
        "every configured provider credential was obtained, every active subscription redeemed, and every active subscription account carries the agent tag that makes it routable"
    };
    ReadinessReport {
        status,
        body: json!({
            "ready": ready,
            "reason": reason,
            "providers": checked,
            "denied": denied,
            "routing": routable,
            "unroutable": unroutable,
            "subscriptions": subscriptions,
            "unredeemable": unredeemable,
            "unroutable_accounts": untagged,
            "operator_action_required": !ready,
            "build": crate::build_info::current(),
        }),
    }
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
    let account_agent = account_agent_id(&client_identity).ok();
    let signed_catalog_agent = if account_agent.is_none() && has_caller_auth_headers(&headers) {
        Some(authorize_caller(&client_identity, &headers, &[], None).await?)
    } else {
        None
    };
    let account_catalog = account_agent.is_some();
    let catalog_agent = account_agent.clone().or(signed_catalog_agent);
    // The same condition decides two disclosures: performance history and
    // whether a model can actually be served. Both are answers about the
    // caller, so neither is given to an unknown caller.
    let caller_known = catalog_agent.is_some();
    // Deployment-owned capabilities never make a route available to a user
    // account. Account availability comes only from that account's stored key.
    let configured_providers = if !account_catalog && catalog_agent.is_some() {
        crate::gateway::broker::configured_provider_capabilities()
    } else {
        HashSet::new()
    };
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
                if catalog_agent.is_some() && configured_providers.contains(&model.provider_id) {
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
    // The desktop console holds a bearer, not an agent signature, so the branch
    // above never runs for it and its catalogue arrived carrying the public
    // vendor list alone. That list knows `openai`; it does not know that a
    // `codex` subscription is what pays for those models here, so no screen in
    // the console could name what a subscription covers.
    if client_identity.client_id == BRAMA_DESKTOP_CLIENT_ID {
        match crate::subscription_dispatch::dispatch::registry_models_for_console().await {
            Ok(models) => {
                for model in models {
                    available.insert(model.route_id.clone());
                    model_ids.push(model.route_id.clone());
                    registry_metadata.insert(model.route_id.clone(), model);
                }
            }
            Err(error) => {
                degraded = true;
                warn!(%error, "console provider model discovery failed");
            }
        }
    }
    // A bearer may be intentionally restricted to a small set of canonical
    // direct routes. Those routes remain real and dispatchable even when the
    // optional public catalogue is unavailable, so the authenticated model list
    // must not become empty while inference still works.
    if let Some(allowed_models) = client_identity.allowed_models.as_ref() {
        for route in allowed_models {
            if crate::providers::adapter::provider_id_from_route(route)
                .is_some_and(crate::gateway::broker::provider_capability_configured)
            {
                model_ids.push(route.clone());
                available.insert(route.clone());
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
    if account_catalog {
        model_ids
            .retain(|model| crate::providers::adapter::provider_id_from_route(model).is_some());
    } else {
        model_ids.retain(|model| client_identity.authorizes_model(model));
    }

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
                // Which provider serves this id. Its absence is why a console
                // could list thousands of models and still not say which of
                // them any one provider or subscription covers: the client had
                // no field to group them by.
                //
                // `route` is sent only when it differs from the id. It matches
                // for all but a handful of a 6,700-model catalogue, and 276 kB
                // of repeating the id back is worth more than the symmetry.
                if let Some(model) = registry {
                    entry["provider"] = json!(model.provider_id);
                    if model.route_id != id {
                        entry["route"] = json!(model.route_id);
                    }
                }
                if caller_known && available.contains(&id) {
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
            // Whether this gateway can serve the id, for a caller whose
            // identity makes the answer knowable. `data` is the public
            // models.dev catalogue, several thousand ids wide, and almost none
            // of them have a credential behind them on any one installation.
            // A caller with no way to tell them apart picks a plausible name
            // and gets `dependency_unavailable` at dispatch -- which is how a
            // downstream product came to default to a model that had never
            // been servable here. The Jeden schema has carried this field all
            // along; the OpenAI-shaped view had nowhere to say it.
            if caller_known {
                entry["available"] = json!(available.contains(&id));
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

fn require_brama_desktop(identity: &ModelClientIdentity) -> Result<(), ApiError> {
    if identity.client_id == BRAMA_DESKTOP_CLIENT_ID && identity.allowed_models.is_none() {
        Ok(())
    } else {
        Err(api_error(StatusCode::FORBIDDEN, "forbidden"))
    }
}

/// Which alias may carry which kind of route. `best` is a chat route like the
/// other chat aliases; what differs is who pays for it, not what it is.
///
/// The five `wisent-backend/*` aliases keep exact shapes because their names
/// are a promise to the caller: whoever asks for `embeddings` must never be
/// handed a chat model. Every other name is the operator's to invent — `smol`,
/// `dumb`, `best-vision` — and carries no such promise, so it is accepted on
/// the general-purpose shape. Rejecting unknown names outright, as this did,
/// made every new alias a Rust change and a gateway release.
///
/// A route naming `best` is delegation rather than a provider: the alias hands
/// the choice to subscription dispatch, which resolves it per caller identity.
fn alias_route_shape_supported(alias: &str, route: &str) -> bool {
    if route == BEST_ALIAS {
        return alias != WISENT_EMBEDDING_ALIAS && alias != WISENT_MODERATION_ALIAS;
    }
    match alias {
        WISENT_EMBEDDING_ALIAS => crate::providers::adapter::supports_embedding_route(route),
        WISENT_MODERATION_ALIAS => crate::providers::adapter::supports_moderation_route(route),
        _ => crate::providers::adapter::supports_chat_route(route),
    }
}

/// Whether this alias must resolve to a provider Brama holds a direct
/// credential for.
///
/// `best` resolves to a subscription route: the caller's HMAC identity selects
/// the subscription that pays, and Brama deliberately holds no direct provider
/// credential for it. Requiring a configured direct capability would reject the
/// only configuration the alias is ever meant to have.
///
/// The same is true of any alias whose route delegates to `best`, which is why
/// this asks about the route as well as the name: an operator-defined alias
/// pointing at the subscription route owns no direct credential either, and
/// keying the exemption on the alias name alone made `best` the only alias that
/// could ever reach a subscription-funded model.
fn alias_requires_direct_capability(alias: &str, route: &str) -> bool {
    alias != BEST_ALIAS && route != BEST_ALIAS
}

fn route_supported(alias: &str, route: &str) -> bool {
    if route.is_empty()
        || route.trim() != route
        || route.contains('*')
        || route.bytes().any(|byte| byte.is_ascii_control())
    {
        return false;
    }
    alias_route_shape_supported(alias, route)
        && (!alias_requires_direct_capability(alias, route)
            || crate::providers::adapter::provider_id_from_route(route)
                .is_some_and(crate::gateway::broker::provider_capability_configured))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AdminRouteUpdate {
    alias: String,
    primary: String,
    #[serde(default)]
    fallbacks: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AdminRouteDelete {
    alias: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AdminCredentialMutation {
    provider: String,
    #[serde(default)]
    credential: Option<String>,
}

fn valid_alias(alias: &str) -> bool {
    !alias.is_empty()
        && alias.len() <= 128
        && alias.trim() == alias
        && alias.bytes().all(|byte| {
            byte.is_ascii_lowercase()
                || byte.is_ascii_digit()
                || matches!(byte, b'-' | b'_' | b'.' | b'/')
        })
}

async fn admin_snapshot(
    Extension(client_identity): Extension<ModelClientIdentity>,
    Extension(aliases): Extension<ModelAliases>,
) -> Result<Json<Value>, ApiError> {
    require_brama_desktop(&client_identity)?;
    let routes = match aliases.routes_file.as_deref() {
        Some(path) => crate::core::inference_routes::snapshot(path).map_err(|_| {
            api_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "route registry unavailable",
            )
        })?,
        None => json!({
            "routes": aliases.routes,
            "fallbacks": {},
            "deployments": [],
        }),
    };
    let providers = crate::providers::adapter::providers()
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
    Ok(Json(json!({
        "schemaVersion": 1,
        "build": crate::build_info::current(),
        "routes": routes,
        "providers": providers,
        "automaticRollback": true,
        "boundaries": {
            "routing": "brama",
            "releases": "stado",
            "credentials": "skarbiec",
        },
    })))
}

async fn update_admin_route(
    Extension(client_identity): Extension<ModelClientIdentity>,
    Extension(aliases): Extension<ModelAliases>,
    Json(request): Json<AdminRouteUpdate>,
) -> Result<Json<Value>, ApiError> {
    require_brama_desktop(&client_identity)?;
    if !valid_alias(&request.alias) {
        return Err(api_error(StatusCode::BAD_REQUEST, "invalid route alias"));
    }
    let mut seen = HashSet::from([request.primary.as_str()]);
    if !route_supported(&request.alias, &request.primary)
        || request
            .fallbacks
            .iter()
            .any(|route| !seen.insert(route.as_str()) || !route_supported(&request.alias, route))
    {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "route chain is unsupported, duplicated, or unavailable",
        ));
    }
    let path = aliases.routes_file.as_deref().ok_or_else(|| {
        api_error(
            StatusCode::CONFLICT,
            "runtime route registry is not configured",
        )
    })?;
    let routes = crate::core::inference_routes::update_route(
        path,
        &request.alias,
        &request.primary,
        &request.fallbacks,
    )
    .map_err(|_| api_error(StatusCode::CONFLICT, "route update was rejected"))?;
    Ok(Json(json!({"ok": true, "routes": routes})))
}

async fn delete_admin_route(
    Extension(client_identity): Extension<ModelClientIdentity>,
    Extension(aliases): Extension<ModelAliases>,
    Json(request): Json<AdminRouteDelete>,
) -> Result<Json<Value>, ApiError> {
    require_brama_desktop(&client_identity)?;
    if !valid_alias(&request.alias) {
        return Err(api_error(StatusCode::BAD_REQUEST, "invalid route alias"));
    }
    if MODEL_ALIASES.contains(&request.alias.as_str()) {
        return Err(api_error(
            StatusCode::CONFLICT,
            "required route aliases cannot be deleted",
        ));
    }
    let path = aliases.routes_file.as_deref().ok_or_else(|| {
        api_error(
            StatusCode::CONFLICT,
            "runtime route registry is not configured",
        )
    })?;
    let routes =
        crate::core::inference_routes::delete_route(path, &request.alias).map_err(|error| {
            if error == "route alias not found" {
                api_error(StatusCode::NOT_FOUND, &error)
            } else {
                api_error(StatusCode::CONFLICT, "route deletion was rejected")
            }
        })?;
    Ok(Json(json!({"ok": true, "routes": routes})))
}

async fn list_admin_credentials(
    Extension(client_identity): Extension<ModelClientIdentity>,
) -> Result<Json<Value>, ApiError> {
    require_brama_desktop(&client_identity)?;
    let providers = crate::gateway::broker::local_provider_names().map_err(|_| {
        api_error(
            StatusCode::CONFLICT,
            "standalone credential store is not enabled",
        )
    })?;
    Ok(Json(json!({"providers": providers})))
}

async fn put_admin_credential(
    Extension(client_identity): Extension<ModelClientIdentity>,
    Json(request): Json<AdminCredentialMutation>,
) -> Result<Json<Value>, ApiError> {
    require_brama_desktop(&client_identity)?;
    let provider = account_credential_provider(Some(&request.provider))
        .await
        .ok_or_else(|| {
            api_error(
                StatusCode::BAD_REQUEST,
                "provider must name a supported remote API provider",
            )
        })?;
    let credential = request.credential.as_deref().unwrap_or("");
    crate::gateway::broker::put_local_provider_credential(&provider, credential)
        .map_err(|error| api_error(StatusCode::BAD_REQUEST, &error))?;
    Ok(Json(json!({"ok": true, "provider": provider})))
}

async fn delete_admin_credential(
    Extension(client_identity): Extension<ModelClientIdentity>,
    Json(request): Json<AdminCredentialMutation>,
) -> Result<Json<Value>, ApiError> {
    require_brama_desktop(&client_identity)?;
    let removed = crate::gateway::broker::remove_local_provider_credential(&request.provider)
        .map_err(|_| {
            api_error(
                StatusCode::CONFLICT,
                "standalone credential store is not enabled",
            )
        })?;
    if !removed {
        return Err(api_error(
            StatusCode::NOT_FOUND,
            "provider credential not found",
        ));
    }
    Ok(Json(json!({"ok": true, "provider": request.provider})))
}

fn valid_agent_id(agent_id: &str) -> bool {
    !agent_id.is_empty()
        && agent_id
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

fn account_agent_id(identity: &ModelClientIdentity) -> Result<String, ApiError> {
    let user_id = identity
        .user_id
        .as_deref()
        .filter(|_| identity.client_id == BRAMA_USER_CLIENT_ID)
        .ok_or_else(|| api_error(StatusCode::FORBIDDEN, "forbidden"))?;
    Ok(format!("user-{}", user_id.replace('-', "")))
}

async fn account_agent_for_route(identity: &ModelClientIdentity, route: &str) -> Option<String> {
    let agent_id = account_agent_id(identity).ok()?;
    let provider = crate::providers::adapter::provider_id_from_route(route)?;
    crate::gateway::broker::list_subscriptions(&agent_id)
        .await
        .into_iter()
        .any(|entry| {
            entry.provider.eq_ignore_ascii_case(provider)
                && entry.status == "active"
                && !crate::journal::is_retired(&entry.id)
        })
        .then_some(agent_id)
}

/// One subscription as every reader sees it: identity, plan windows the
/// provider reported, where those windows came from and whether they are still
/// current, what Brama measured, any block in force, when this record last
/// changed, what the newest proactive check learned, and where its credential
/// stands with the provider.
///
/// `limits` being empty is not one state but four, and the newest check and the
/// record instant are what separate them. Windows present: render them, aged by
/// the newest `recorded_at_ms`, and say "as of" only when `stale` is false.
/// Empty with a failed check: the credential or the provider said no, and
/// `probe.detail` is its sentence. Empty with no check and nothing measured:
/// nothing has ever gone through this subscription. Empty with a successful
/// check: the provider genuinely publishes no plan window, and only this state
/// may be rendered as such. A zero is never one of the four.
///
/// `usage_source` says which of the three statements the newest window is -- the
/// provider's own usage report, the headers of real traffic, or an operator's
/// probe -- and is null when there is no window to attribute. `stale` says the
/// newest window has aged past the freshness window; a stale reading is still
/// served, because a number with its own instant beside it is information and an
/// empty plan is not.
fn subscription_view(entry: &crate::gateway::broker::SubscriptionEntry) -> Value {
    let recorded = crate::subscription_dispatch::usage::usage_for(&entry.id);
    let windows = crate::subscription_dispatch::usage::plan_windows(recorded.as_ref());
    json!({
        "id": entry.id,
        "provider": entry.provider,
        "status": entry.status,
        "label": entry.label,
        "limits": windows.limits,
        "measured": recorded.as_ref().map(|usage| &usage.measured),
        "block": recorded.as_ref().and_then(|usage| usage.block.clone()),
        "observed_at_ms": recorded.as_ref().and_then(|usage| usage.updated_at_ms),
        "probe": recorded.as_ref().and_then(|usage| usage.probe.clone()),
        "credential": credential_view(entry, recorded.as_ref()),
        "usage_source": windows.source.map(|source| source.as_str()),
        "stale": windows.stale,
    })
}

/// Where one subscription's credential stands, or `null` when nothing has ever
/// been recorded about its grant.
///
/// A retirement outranks whatever the last refresh concluded. An operator who
/// retired a subscription said it must not be used, and reporting it as
/// `needs_reauthorization` would invite a sign-in that changes nothing; a
/// retirement recorded by an older build left no credential record at all, which
/// is the case this override exists for.
///
/// Every field is present whenever the object is, and `state` is the only one
/// that is never null: a reader that has to guess which of the three states an
/// absent field meant is exactly the reading that let four refused credentials
/// pass for quiet ones.
fn credential_view(
    entry: &crate::gateway::broker::SubscriptionEntry,
    recorded: Option<&crate::subscription_dispatch::usage::SubscriptionUsage>,
) -> Value {
    use crate::subscription_dispatch::usage::CredentialState;

    let Some(credential) = recorded.and_then(|usage| usage.credential.as_ref()) else {
        return Value::Null;
    };
    let retired = entry.status != "active" || crate::journal::is_retired(&entry.id);
    let state = if retired {
        CredentialState::Disabled
    } else {
        credential.state
    };
    json!({
        "state": state.as_str(),
        "cause": credential.cause,
        "recorded_at_ms": credential.recorded_at_ms,
        "expires_at_ms": credential.expires_at_ms,
        "refreshed_at_ms": credential.refreshed_at_ms,
    })
}

async fn list_subscriptions(agent_id: String) -> Result<Json<Value>, ApiError> {
    if !valid_agent_id(&agent_id) {
        return Err(api_error(StatusCode::BAD_REQUEST, "invalid agent id"));
    }
    let subscriptions = crate::gateway::broker::list_subscriptions(&agent_id)
        .await
        .into_iter()
        .map(|entry| subscription_view(&entry))
        .collect::<Vec<_>>();
    Ok(Json(
        json!({"agentId": agent_id, "subscriptions": subscriptions}),
    ))
}

async fn account_credential_provider(value: Option<&str>) -> Option<String> {
    let provider = match value.map(str::trim) {
        Some("claude_code") => "claude-code",
        Some(provider) if !provider.is_empty() => provider,
        _ => return None,
    };
    if provider == "local-openai" {
        return None;
    }
    if crate::providers::adapter::provider(provider).is_some() {
        return Some(provider.to_string());
    }
    crate::subscription_dispatch::model_catalog::snapshot()
        .await
        .ok()?
        .providers
        .get(provider)
        .filter(|provider| provider.executable())
        .map(|provider| provider.id.clone())
}

async fn create_subscription(
    agent_id: String,
    request: DonateSubscriptionRequest,
) -> Result<Json<Value>, ApiError> {
    if !valid_agent_id(&agent_id) {
        return Err(api_error(StatusCode::BAD_REQUEST, "invalid agent id"));
    }
    let provider = account_credential_provider(request.provider.as_deref())
        .await
        .ok_or_else(|| {
            api_error(
                StatusCode::BAD_REQUEST,
                "provider must name a supported remote API or subscription provider",
            )
        })?;
    let api_key = request.api_key.as_deref().unwrap_or("");
    if api_key.is_empty() || api_key.chars().count() > 8000 {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "api_key must contain 1..8000 characters",
        ));
    }
    // One row per agent and provider, refreshed in place. A per-donation id
    // needed a new entry in the operator's routes table before anything could
    // read it, so donations accumulated rows that no request could use.
    let subscription_id = format!(
        "brama-sub-{}-{}-primary",
        crate::gateway::broker::slug(&agent_id),
        crate::gateway::broker::slug(&provider)
    );
    // A document that carries no credential is the donor's mistake and the vault
    // was not touched, so it is a 400 naming the shape rather than a conflict:
    // this coordinate is the only copy of that subscription's credential, and the
    // previous answer here made destroying it and failing to write it read alike.
    crate::gateway::broker::put_donated_credential(&provider, &subscription_id, api_key)
        .await
        .map_err(|refusal| match refusal {
            crate::gateway::broker::DonationRefusal::Unusable(detail) => {
                api_error(StatusCode::BAD_REQUEST, &detail)
            }
            crate::gateway::broker::DonationRefusal::Unwritable(_) => {
                api_error(StatusCode::CONFLICT, "subscription credential was rejected")
            }
        })?;
    crate::gateway::broker::donated_add(
        &agent_id,
        &subscription_id,
        &provider,
        request.label.as_deref(),
    )
    .map_err(|_| api_error(StatusCode::CONFLICT, "subscription registration failed"))?;
    Ok(Json(json!({
        "subscription": {
            "id": subscription_id,
            "provider": provider,
            "status": "active",
            "label": request.label,
        }
    })))
}

async fn retire_managed_subscription(
    agent_id: String,
    subscription_id: String,
) -> Result<Json<Value>, ApiError> {
    let owned = crate::gateway::broker::list_subscriptions(&agent_id)
        .await
        .into_iter()
        .find(|entry| entry.id == subscription_id);
    let Some(owned) = owned else {
        return Err(api_error(StatusCode::NOT_FOUND, "subscription not found"));
    };
    crate::journal::retire(&subscription_id);
    // Retirement is recorded in the ledger as well as the journal, so a row can
    // say `disabled` with the instant and the reason it happened rather than
    // leaving a reader to infer a retirement from an absence.
    crate::subscription_dispatch::usage::record_credential_disabled(
        &subscription_id,
        &owned.provider,
        "retired by its owning agent",
    );
    crate::gateway::broker::donated_remove(&subscription_id)
        .map_err(|_| api_error(StatusCode::CONFLICT, "subscription retirement failed"))?;
    crate::gateway::broker::remove_donated_credential(&owned.provider, &subscription_id).map_err(
        |_| {
            api_error(
                StatusCode::CONFLICT,
                "subscription credential retirement failed",
            )
        },
    )?;
    Ok(Json(json!({"ok": true})))
}

async fn list_account_subscriptions(
    Extension(client_identity): Extension<ModelClientIdentity>,
) -> Result<Json<Value>, ApiError> {
    list_subscriptions(account_agent_id(&client_identity)?).await
}

async fn create_account_subscription(
    Extension(client_identity): Extension<ModelClientIdentity>,
    Json(request): Json<DonateSubscriptionRequest>,
) -> Result<Json<Value>, ApiError> {
    create_subscription(account_agent_id(&client_identity)?, request).await
}

async fn retire_account_subscription(
    Extension(client_identity): Extension<ModelClientIdentity>,
    Path(subscription_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    retire_managed_subscription(account_agent_id(&client_identity)?, subscription_id).await
}

async fn list_admin_subscriptions(
    Extension(client_identity): Extension<ModelClientIdentity>,
    Path(agent_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    require_brama_desktop(&client_identity)?;
    list_subscriptions(agent_id).await
}

async fn create_admin_subscription(
    Extension(client_identity): Extension<ModelClientIdentity>,
    Path(agent_id): Path<String>,
    Json(request): Json<DonateSubscriptionRequest>,
) -> Result<Json<Value>, ApiError> {
    require_brama_desktop(&client_identity)?;
    create_subscription(agent_id, request).await
}

async fn retire_admin_subscription(
    Extension(client_identity): Extension<ModelClientIdentity>,
    Path((agent_id, subscription_id)): Path<(String, String)>,
) -> Result<Json<Value>, ApiError> {
    require_brama_desktop(&client_identity)?;
    retire_managed_subscription(agent_id, subscription_id).await
}

/// Spend one minimal completion against one subscription, because an operator
/// asked whether the provider will actually serve it.
///
/// Nothing else in the gateway spends quota to learn a statistic: plan windows
/// arrive from each provider's own free usage report. This is the deliberate
/// exception, and it is a route rather than a subcommand because redeeming the
/// credential needs the capabilities and identities the launcher installed in
/// this serving process -- a standalone desktop install holds its provider
/// credentials only in this process's memory -- and because the console that
/// renders the verdict is where the question is asked.
///
/// A refusal to spend is a `409`, not a `500`: an account inside a recorded
/// rate-limit block already told us it is out of quota, and the block exists to
/// stop us paying to hear it twice.
async fn probe_admin_subscription(
    Extension(client_identity): Extension<ModelClientIdentity>,
    Path((agent_id, subscription_id)): Path<(String, String)>,
) -> Result<Json<Value>, ApiError> {
    require_brama_desktop(&client_identity)?;
    if !valid_agent_id(&agent_id) {
        return Err(api_error(StatusCode::BAD_REQUEST, "invalid agent id"));
    }
    let entry = crate::gateway::broker::list_subscriptions(&agent_id)
        .await
        .into_iter()
        .find(|entry| entry.id == subscription_id)
        .ok_or_else(|| api_error(StatusCode::NOT_FOUND, "subscription not found"))?;
    let probe = crate::subscription_dispatch::probe::probe_once(&entry.id, &entry.provider)
        .await
        .map_err(|message| api_error(StatusCode::CONFLICT, &message))?;
    Ok(Json(json!({
        "ok": true,
        "probe": probe,
        "subscription": subscription_view(&entry),
    })))
}
/// The dispatch pool exactly as the process that dispatches requests believes
/// it: the same document `pool::report` builds for the operator-facing
/// listing, served by the process that owns the ledger instead of a second
/// process asked to reconstruct it.
///
/// It is not mounted under `/v1/admin/subscriptions/pool` because this
/// router's matcher (matchit 0.7) refuses a static segment beside the
/// `:agent_id` parameter already registered there.
async fn admin_subscription_pool(
    Extension(client_identity): Extension<ModelClientIdentity>,
) -> Result<Json<Value>, ApiError> {
    require_brama_desktop(&client_identity)?;
    Ok(Json(crate::subscription_dispatch::pool::report().await))
}

/// One operator-driven refresh of a provider's pooled grants, run by the
/// serving process so the attempt shares the sweep's code path and its audit
/// record. A verdict whose result is not `refreshed` is still a 200: the body
/// is the report the operator came for, and the failure it names belongs to
/// the provider, not to this endpoint.
async fn refresh_admin_subscription_pool(
    Extension(client_identity): Extension<ModelClientIdentity>,
    Json(request): Json<RefreshSubscriptionPoolRequest>,
) -> Result<Json<Value>, ApiError> {
    require_brama_desktop(&client_identity)?;
    crate::subscription_dispatch::pool::refresh_provider(
        &request.provider.unwrap_or_default(),
        &request.reason.unwrap_or_default(),
    )
    .await
    .map(Json)
    .map_err(|message| api_error(StatusCode::BAD_REQUEST, &message))
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
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RefreshSubscriptionPoolRequest {
    provider: Option<String>,
    reason: Option<String>,
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
        // The response stays deliberately blank -- an unauthenticated caller
        // learns nothing -- but the operator was learning nothing either. A
        // missing header, a clock outside the skew window, a secret the gateway
        // could not redeem and a genuinely wrong signature all arrived as one
        // word, and telling them apart from the outside is not possible: the
        // caller sees 401 whether the fault is its own or this side's.
        .map_err(|reason| {
            warn!(%reason, event = "caller_auth_rejected", "agent authentication refused");
            api_error(StatusCode::UNAUTHORIZED, "unauthorized")
        })?;
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
        .map(|entry| subscription_view(&entry))
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
    // Account and agent credentials share one provider contract. A provider
    // accepted here is executable by the same static or catalogue adapter the
    // request path will later use.
    let provider = match account_credential_provider(req.provider.as_deref()).await {
        Some(provider) => provider,
        None => {
            return api_error(
                StatusCode::BAD_REQUEST,
                "provider must name a supported remote API or subscription provider",
            )
        }
    };
    let api_key = req.api_key.as_deref().unwrap_or("");
    if api_key.is_empty() || api_key.chars().count() > 8000 {
        return api_error(
            StatusCode::BAD_REQUEST,
            "api_key must contain 1..8000 characters",
        );
    }
    // One row per agent and provider, refreshed in place: reads resolve through
    // the operator's routes table, and a per-donation id has no entry there.
    let subscription_id = format!(
        "brama-sub-{}-{}-primary",
        crate::gateway::broker::slug(&agent_id),
        crate::gateway::broker::slug(&provider)
    );
    // The donor learns which of the two happened: a document carrying no
    // credential leaves the stored one untouched and is the caller's to fix,
    // while a failed write is this installation's.
    if let Err(refusal) =
        crate::gateway::broker::put_donated_credential(&provider, &subscription_id, api_key).await
    {
        let status = match refusal {
            crate::gateway::broker::DonationRefusal::Unusable(_) => StatusCode::BAD_REQUEST,
            crate::gateway::broker::DonationRefusal::Unwritable(_) => {
                StatusCode::INTERNAL_SERVER_ERROR
            }
        };
        return api_error(status, refusal.detail());
    }
    if let Err(message) = crate::gateway::broker::donated_add(
        &agent_id,
        &subscription_id,
        &provider,
        req.label.as_deref(),
    ) {
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
        .find(|entry| entry.id == subscription_id);
    let Some(owned) = owned else {
        return api_error(StatusCode::NOT_FOUND, "subscription not found");
    };
    crate::journal::retire(subscription_id);
    // The same record the managed path writes: a retired row states `disabled`
    // with the instant it happened instead of an empty credential object.
    crate::subscription_dispatch::usage::record_credential_disabled(
        subscription_id,
        &owned.provider,
        "retired by an operator",
    );
    if let Err(message) = crate::gateway::broker::donated_remove(subscription_id) {
        return api_error(StatusCode::INTERNAL_SERVER_ERROR, &message);
    }
    if let Err(message) =
        crate::gateway::broker::remove_donated_credential(&owned.provider, subscription_id)
    {
        return api_error(StatusCode::INTERNAL_SERVER_ERROR, &message);
    }
    (StatusCode::OK, Json(json!({"ok": true})))
}

pub async fn start_server(port: u16, standalone: bool) -> Result<(), std::io::Error> {
    let _ = STARTED_AT.elapsed();
    let ingress_auth = ModelIngressAuth::from_env()?;
    if !standalone {
        ingress_auth.requires_exact_aliases("wisent-backend", WISENT_MODEL_ALIASES)?;
        // Weles drafts browser trajectories, which needs a frontier
        // instruction-following model rather than whatever local deployment
        // happens to be up. `best` is the subscription route: the caller's
        // HMAC identity selects the subscription that pays, and it is the only
        // alias exempt from `alias_requires_direct_capability`, so it is also
        // the only way this client can reach a subscription-funded model.
        //
        // It is the ONLY alias this client gets. A second route was tried and
        // withdrawn: it can only point at a direct-credential deployment, and
        // sending browser work to whatever model happens to be up is worse than
        // not running it. When this pool is empty the answer is to refill it.
        ingress_auth.requires_exact_aliases("weles", &[BEST_ALIAS])?;
    }
    let aliases = ModelAliases::from_env(!standalone)?;
    // Touch the perf registry so persisted stats load at startup, not on first use.
    info!(
        models = crate::core::perf::tracked_count(),
        "perf registry loaded"
    );
    // Readiness crosses the local capability broker and provider discovery. It
    // runs out of band so a deploy probe always receives the latest completed
    // answer instead of canceling an in-flight check at its HTTP deadline.
    spawn_readiness_probe();
    // Plan usage is read from each provider's own usage report on a timer rather
    // than only from whatever traffic happens to arrive, so a subscription
    // nobody routed through today still reports what its plan says -- and no
    // timer spends a completion to find out.
    crate::subscription_dispatch::plan_usage::spawn();
    // A grant is replaced before it expires rather than when a request trips
    // over it, and a grant the provider has disowned is recorded the first time
    // it says so instead of being rediscovered by every later request.
    crate::subscription_dispatch::refresh_sweep::spawn();

    let protected = Router::new()
        .route("/v1/chat/completions", post(chat_completions))
        // The two first-party formats callers already speak. Same routing
        // decision, same identities, same bounded attempts as the line above.
        .route("/v1/messages", post(anthropic_messages))
        .route("/v1/responses", post(openai_responses))
        .route("/v1/embeddings", post(embeddings))
        .route("/v1/moderations", post(moderations))
        .route("/v1/models", get(list_models))
        .route(
            "/v1/subscriptions/:agent_id",
            get(list_agent_subscriptions)
                .post(donate_subscription)
                .delete(retire_subscription),
        )
        .route(
            "/v1/account/subscriptions",
            get(list_account_subscriptions).post(create_account_subscription),
        )
        .route(
            "/v1/account/subscriptions/:subscription_id",
            delete(retire_account_subscription),
        )
        .route("/stats", get(get_stats))
        .route("/v1/admin/snapshot", get(admin_snapshot))
        .route(
            "/v1/admin/routes",
            put(update_admin_route).delete(delete_admin_route),
        )
        .route(
            "/v1/admin/credentials",
            get(list_admin_credentials)
                .put(put_admin_credential)
                .delete(delete_admin_credential),
        )
        .route(
            "/v1/admin/subscriptions/:agent_id",
            get(list_admin_subscriptions).post(create_admin_subscription),
        )
        .route(
            "/v1/admin/subscriptions/:agent_id/:subscription_id",
            delete(retire_admin_subscription),
        )
        .route(
            "/v1/admin/subscriptions/:agent_id/:subscription_id/probe",
            post(probe_admin_subscription),
        )
        .route("/v1/admin/subscription-pool", get(admin_subscription_pool))
        .route(
            "/v1/admin/subscription-pool/refresh",
            post(refresh_admin_subscription_pool),
        )
        .layer(Extension(aliases))
        .layer(middleware::from_fn_with_state(
            ingress_auth,
            require_model_bearer,
        ));
    let app = Router::new()
        .route("/health", get(health))
        .route("/readyz", get(readyz))
        .merge(protected)
        .layer(middleware::from_fn(require_secure_transport));

    // Loopback unless this host is told otherwise. A gateway that binds a
    // routable address by default is one that can be reached before anybody
    // decided it should be, so the placed host declares its own address and
    // every other host keeps the old behaviour.
    let addr = match std::env::var("BRAMA_BIND_ADDRESS") {
        Ok(configured) if !configured.trim().is_empty() => {
            let parsed: std::net::IpAddr = configured.trim().parse().map_err(|_| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!("BRAMA_BIND_ADDRESS is not an IP address: {configured}"),
                )
            })?;
            SocketAddr::from((parsed, port))
        }
        _ => SocketAddr::from((Ipv4Addr::LOCALHOST, port)),
    };
    info!("Starting brama server on {addr}");

    let listener = tokio::net::TcpListener::bind(addr).await?;
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
