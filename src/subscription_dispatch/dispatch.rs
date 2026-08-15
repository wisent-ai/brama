//! Route canonical `provider/model` requests through stateless provider APIs.

use axum::http::HeaderMap;
use serde_json::Value;
use std::collections::HashMap;
use std::io::Read;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};
use tracing::{info, warn};

use crate::core::failure::{
    self, IMPACT_MODEL_REQUEST, POINT_BOUNDED_ROTATION, POINT_CREDENTIAL_SELECTION,
    POINT_MODEL_SELECTION,
};
use crate::crypto;
use crate::gateway::broker;
use crate::providers::adapter as provider_registry;
use crate::subscription_dispatch::usage;
use crate::types::{ModelRequest, ModelResponse};
use wisent_errors::Failure;

/// The envelope for one refused model request: where it broke, what the caller
/// loses, the reason verbatim, and the failure underneath it when there is one.
fn refusal_envelope(model: &str, point: &str, message: &str, cause: Option<Failure>) -> Failure {
    let refusal = failure::envelope(
        point,
        // Brama has always told its clients that an unavailable subscription is
        // worth retrying. The code is looked up from that same kind rather than
        // chosen here, so it cannot come to mean something else than it does at
        // the HTTP edge.
        failure::code_for("subscription_unavailable"),
        IMPACT_MODEL_REQUEST,
        message,
    )
    .with_context("model", model);
    match cause {
        Some(cause) => refusal.caused_by(cause),
        None => refusal,
    }
}

/// Refuse one model request, saying where it broke and what the layer below
/// said, and hand the caller the sentence it has always been handed.
///
/// The message is the envelope's detail and nothing else: clients parse these
/// strings, so the envelope travels in the log beside them, never inside them.
fn refuse(
    request: &ModelRequest,
    point: &str,
    message: String,
    cause: Option<Failure>,
) -> ModelResponse {
    let refusal = refusal_envelope(&request.model, point, &message, cause);
    warn!(
        event = "dispatch_refused",
        model = %request.model,
        envelope = %refusal.to_json(),
        "{}",
        refusal.render()
    );
    ModelResponse::failure(&request.model, message)
}

fn max_selector_models() -> usize {
    "3".parse().expect("valid selector model limit")
}

fn max_credential_attempts() -> usize {
    "2".parse().expect("valid credential attempt limit")
}
const MODEL_CACHE_TTL: Duration = Duration::from_secs(300);
fn is_permanent_auth_failure(error: &str) -> bool {
    let error = error.to_ascii_lowercase();
    error.contains("refresh token was revoked")
        || error.contains("access token could not be refreshed")
        || error.contains("invalid_grant")
}

fn is_auth_failure(error: &str) -> bool {
    let error = error.to_ascii_lowercase();
    error.contains("invalid authentication")
        || error.contains("authentication_error")
        || error.contains("failed to authenticate")
        || error.contains("401")
        || error.contains("provider_authentication")
        || error.contains("oauth")
        || is_permanent_auth_failure(&error)
}

async fn mark_credential_revoked(credential_id: &str) {
    crate::journal::retire(credential_id);
}

struct CachedRegistryModels {
    fetched: Instant,
    models: Vec<provider_registry::RegistryModel>,
}

static REGISTRY_MODEL_CACHE: LazyLock<Mutex<HashMap<String, CachedRegistryModels>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Discovery failures are cached briefly too: without it every catalog call
/// re-pays full provider timeouts for credentials that are stale anyway.
const MODEL_FAILURE_CACHE_TTL: Duration = Duration::from_secs(60);
static REGISTRY_MODEL_FAILURE_CACHE: LazyLock<Mutex<HashMap<String, (Instant, String)>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

pub fn is_subscription_model(model: &str) -> bool {
    provider_for(model).is_some()
}

pub fn provider_requires_caller_identity(model: &str) -> bool {
    matches!(provider_for(model), Some("claude-code" | "codex" | "kimi"))
}

pub(crate) fn provider_for(model: &str) -> Option<&str> {
    provider_registry::provider_id_from_route(model)
}

pub async fn registry_models_for_agent(
    agent_id: &str,
) -> Result<Vec<provider_registry::RegistryModel>, String> {
    let entries = broker::list_subscriptions(agent_id)
        .await
        .into_iter()
        .filter(|entry| entry.status == "active" && !crate::journal::is_retired(&entry.id))
        .collect::<Vec<_>>();
    if entries.is_empty() {
        return Ok(Vec::new());
    }

    let mut models_by_route = HashMap::new();
    let mut failures = Vec::new();
    for entry in entries {
        let provider = entry.provider.trim();
        let cache_key = format!("{provider}:{}", entry.id);
        let cached = REGISTRY_MODEL_CACHE.lock().ok().and_then(|cache| {
            cache
                .get(&cache_key)
                .filter(|item| item.fetched.elapsed() < MODEL_CACHE_TTL)
                .map(|item| item.models.clone())
        });
        let models = if let Some(cached) = cached {
            cached
        } else {
            let recent_failure = REGISTRY_MODEL_FAILURE_CACHE.lock().ok().and_then(|cache| {
                cache
                    .get(&cache_key)
                    .filter(|(fetched, _)| fetched.elapsed() < MODEL_FAILURE_CACHE_TTL)
                    .map(|(_, error)| error.clone())
            });
            if let Some(error) = recent_failure {
                failures.push(format!("{}: {error}", entry.id));
                continue;
            }
            let secret = match broker::subscription_credential(&entry.id, provider).await {
                Some(secret) => secret,
                None => {
                    failures.push(format!("{}: credential unavailable", entry.id));
                    continue;
                }
            };
            let secret = match secret.expose_utf8() {
                Ok(secret) => secret,
                Err(_) => {
                    failures.push(format!("{}: credential is not UTF-8", entry.id));
                    continue;
                }
            };
            let item = broker::subscription_resource(provider, &entry.id);
            let discovered = match provider_registry::discover_models(provider, &item, secret).await
            {
                Ok(models) => models,
                Err(error) => {
                    if let Ok(mut cache) = REGISTRY_MODEL_FAILURE_CACHE.lock() {
                        cache.insert(cache_key.clone(), (Instant::now(), error.clone()));
                    }
                    failures.push(format!("{}: {error}", entry.id));
                    continue;
                }
            };
            if let Ok(mut cache) = REGISTRY_MODEL_CACHE.lock() {
                cache.insert(
                    cache_key,
                    CachedRegistryModels {
                        fetched: Instant::now(),
                        models: discovered.clone(),
                    },
                );
            }
            discovered
        };
        for model in models {
            models_by_route
                .entry(model.route_id.clone())
                .or_insert(model);
        }
    }
    if models_by_route.is_empty() && !failures.is_empty() {
        return Err(format!(
            "could not discover native provider models: {}",
            failures.join("; ")
        ));
    }
    let mut models = models_by_route.into_values().collect::<Vec<_>>();
    models.sort_by(|left, right| left.route_id.cmp(&right.route_id));
    Ok(models)
}

fn provider_matches(candidate: &str, requested: &str) -> bool {
    candidate.trim().eq_ignore_ascii_case(requested)
}

fn eligible_subscription_entries(
    entries: Vec<broker::SubscriptionEntry>,
    provider: &str,
    target: Option<&crate::types::BillingTarget>,
) -> Result<Vec<broker::SubscriptionEntry>, String> {
    if let Some(target) = target {
        if target.account_id.trim().is_empty() || target.subscription_id.trim().is_empty() {
            return Err("billingTarget accountId and subscriptionId are required".into());
        }
        if !provider_matches(&target.provider_id, provider) {
            return Err(format!(
                "billingTarget provider '{}' does not match model provider '{provider}'",
                target.provider_id
            ));
        }
    }
    Ok(entries
        .into_iter()
        .filter(|entry| {
            provider_matches(&entry.provider, provider)
                && entry.status == "active"
                && !crate::journal::is_retired(&entry.id)
                && target.is_none_or(|target| entry.id == target.subscription_id)
        })
        .collect())
}

pub(crate) async fn authenticate_agent(
    headers: &HeaderMap,
    raw_body: &[u8],
) -> Result<String, String> {
    let agent_id = headers
        .get("x-agent-id")
        .and_then(|v| v.to_str().ok())
        .filter(|s| !s.is_empty())
        .map(String::from)
        .ok_or_else(|| "missing x-agent-id header".to_string())?;
    let ts = headers
        .get("x-agent-timestamp")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let sig = headers
        .get("x-agent-signature")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    let secret = broker::get_agent_auth_secret(&agent_id)
        .await
        .ok_or_else(|| "no auth secret for agent".to_string())?;

    let headers_for_check = crypto::HmacHeaders {
        agent_id: &agent_id,
        timestamp: ts,
        signature: sig,
    };
    crypto::verify_agent_hmac(&headers_for_check, raw_body, secret.expose())
        .map_err(|e| format!("auth: {e}"))?;
    Ok(agent_id)
}

fn random_u64() -> Result<u64, String> {
    let mut bytes = u64::default().to_ne_bytes();
    std::fs::File::open("/dev/urandom")
        .and_then(|mut source| source.read_exact(&mut bytes))
        .map_err(|_| "operating system randomness is unavailable".to_string())?;
    Ok(u64::from_ne_bytes(bytes))
}

fn shuffle_models(models: &mut [String]) -> Result<(), String> {
    let one = std::iter::once(()).count();
    if models.len() <= one {
        return Ok(());
    }
    for i in (one..models.len()).rev() {
        let j = (random_u64()? as usize) % (i + one);
        models.swap(i, j);
    }
    Ok(())
}

async fn any_subscription_models(
    headers: &HeaderMap,
    raw_body: &[u8],
) -> Result<Vec<String>, String> {
    let agent_id = authenticate_agent(headers, raw_body).await?;
    active_supported_models_for_agent(&agent_id).await
}

async fn any_vision_capable_subscription_models(
    headers: &HeaderMap,
    raw_body: &[u8],
) -> Result<Vec<String>, String> {
    let agent_id = authenticate_agent(headers, raw_body).await?;
    active_vision_capable_models_for_agent(&agent_id).await
}

pub async fn active_supported_models_for_agent(agent_id: &str) -> Result<Vec<String>, String> {
    let mut models = registry_models_for_agent(agent_id)
        .await?
        .into_iter()
        .map(|model| model.route_id)
        .collect::<Vec<_>>();
    if models.is_empty() {
        return Err("no active stateless provider models for signed agent".into());
    }
    shuffle_models(&mut models)?;
    Ok(models)
}

pub async fn active_vision_capable_models_for_agent(agent_id: &str) -> Result<Vec<String>, String> {
    let mut models = registry_models_for_agent(agent_id)
        .await?
        .into_iter()
        .filter(|model| model.input_modalities.iter().any(|value| value == "image"))
        .map(|model| model.route_id)
        .collect::<Vec<_>>();
    if models.is_empty() {
        return Err("no active vision-capable stateless provider model for signed agent".into());
    }
    shuffle_models(&mut models)?;
    Ok(models)
}

fn score_field(row: &Value) -> Option<f64> {
    row.get("score").and_then(|score| score.as_f64())
}

fn model_field(row: &Value) -> Option<String> {
    row.get("model")
        .and_then(|model| model.as_str())
        .map(|model| model.trim().to_string())
        .filter(|model| !model.is_empty())
}

fn checked_at_field(row: &Value) -> i64 {
    let value = string_field(row, "checked_at");
    chrono::DateTime::parse_from_rfc3339(&value)
        .map(|dt| dt.timestamp())
        .unwrap_or(0)
}

async fn task_quality_models(agent_id: &str, task: &str) -> Result<Vec<String>, String> {
    let active_models = active_supported_models_for_agent(agent_id).await?;
    let rows = crate::journal::checks_for_task(agent_id, task);
    if rows.is_empty() {
        return Err(format!("no quality checks configured for task '{task}'"));
    }
    let mut latest_by_model: HashMap<String, (f64, i64)> = HashMap::new();
    for row in rows
        .iter()
        .filter(|row| string_field(row, "status") == "active")
    {
        let Some(model) = model_field(row) else {
            continue;
        };
        if !active_models.iter().any(|active| active == &model) {
            continue;
        }
        let Some(score) = score_field(row) else {
            continue;
        };
        let checked_at = checked_at_field(row);
        match latest_by_model.get(&model) {
            Some((_, existing_checked_at)) if *existing_checked_at >= checked_at => {}
            _ => {
                latest_by_model.insert(model, (score, checked_at));
            }
        }
    }
    let mut scored = latest_by_model
        .into_iter()
        .map(|(model, (score, checked_at))| (model, score, checked_at))
        .collect::<Vec<_>>();
    if scored.is_empty() {
        return Err(format!(
            "no active quality check result for task '{task}' and signed agent"
        ));
    }
    scored.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| b.2.cmp(&a.2))
    });
    let mut ordered = Vec::new();
    let mut idx = 0;
    while idx < scored.len() {
        let score = scored[idx].1;
        let mut group = Vec::new();
        while idx < scored.len() && (scored[idx].1 - score).abs() < f64::EPSILON {
            group.push(scored[idx].0.clone());
            idx += 1;
        }
        shuffle_models(&mut group)?;
        ordered.extend(group);
    }
    Ok(ordered)
}

/// `model: "any"` selects among active stateless provider routes for the
/// signed agent and rotates across credentials on provider exhaustion.
async fn dispatch_ranked_models(
    headers: &HeaderMap,
    request: &ModelRequest,
    raw_body: &[u8],
    models: Vec<String>,
    failure_context: String,
) -> ModelResponse {
    let mut attempts = u32::default();
    let mut errors = Vec::new();
    for model in models.into_iter().take(max_selector_models()) {
        let mut candidate = request.clone();
        candidate.model = model.clone();
        let mut response = dispatch_subscription(headers, &candidate, raw_body).await;
        attempts = attempts.saturating_add(response.attempts);
        response.attempts = attempts;
        if response.success {
            return response;
        }
        errors.push(format!(
            "{}: {}",
            model,
            response.error.as_deref().unwrap_or("failed")
        ));
    }
    let mut failure = refuse(
        request,
        POINT_MODEL_SELECTION,
        format!("{failure_context}; tried {}", errors.join("; ")),
        None,
    );
    failure.attempts = attempts;
    failure
}

pub async fn dispatch_any_subscription(
    headers: &HeaderMap,
    request: &ModelRequest,
    raw_body: &[u8],
) -> ModelResponse {
    let models = match any_subscription_models(headers, raw_body).await {
        Ok(models) => models,
        Err(e) => return ModelResponse::failure(&request.model, e),
    };
    dispatch_ranked_models(
        headers,
        request,
        raw_body,
        models,
        "no working subscription model for signed agent".into(),
    )
    .await
}

/// `model: "any-vision-capable"` selects an active stateless provider route
/// whose catalog metadata advertises image input.
pub async fn dispatch_any_vision_capable_subscription(
    headers: &HeaderMap,
    request: &ModelRequest,
    raw_body: &[u8],
) -> ModelResponse {
    let models = match any_vision_capable_subscription_models(headers, raw_body).await {
        Ok(models) => models,
        Err(e) => return ModelResponse::failure(&request.model, e),
    };
    dispatch_ranked_models(
        headers,
        request,
        raw_body,
        models,
        "no working vision-capable subscription model for signed agent".into(),
    )
    .await
}

/// `model: "task:<name>"` means: use measured quality evidence for `<name>`.
/// The router does not infer tasks from prompt text. It only uses persisted
/// rows written by the task-quality collector.
pub async fn dispatch_task_subscription(
    headers: &HeaderMap,
    request: &ModelRequest,
    raw_body: &[u8],
    task: &str,
) -> ModelResponse {
    let agent_id = match authenticate_agent(headers, raw_body).await {
        Ok(id) => id,
        Err(e) => return ModelResponse::failure(&request.model, e),
    };
    let models = match task_quality_models(&agent_id, task).await {
        Ok(models) => models,
        Err(e) => return ModelResponse::failure(&request.model, e),
    };
    dispatch_ranked_models(
        headers,
        request,
        raw_body,
        models,
        format!("no working quality-ranked model for task '{task}'"),
    )
    .await
}

pub async fn dispatch_subscription_for_agent(
    agent_id: &str,
    request: &ModelRequest,
) -> ModelResponse {
    let provider = match provider_for(&request.model) {
        Some(provider) => provider,
        None => {
            return ModelResponse::failure(&request.model, "unknown provider/model route".into())
        }
    };
    dispatch_subscription_attempt(provider, agent_id, request).await
}

/// Execute a caller-independent canonical route with Brama's dedicated direct
/// provider capability. Subscription provider credentials are never eligible.
pub async fn dispatch_direct(request: &ModelRequest) -> ModelResponse {
    let provider = match provider_for(&request.model) {
        Some(provider) => provider,
        None => {
            return ModelResponse::failure(&request.model, "unknown provider/model route".into())
        }
    };
    if provider_requires_caller_identity(&request.model) {
        return ModelResponse::failure(
            &request.model,
            "auth: caller identity is required for subscription providers".into(),
        );
    }
    let credential = match broker::provider_credential(provider).await {
        Some(credential) => credential,
        None => {
            return ModelResponse::failure(
                &request.model,
                format!("direct '{provider}' credential is unavailable"),
            )
        }
    };
    let credential = match credential.expose_utf8() {
        Ok(credential) => credential,
        Err(_) => {
            return ModelResponse::failure(
                &request.model,
                format!("direct '{provider}' credential is not valid UTF-8"),
            )
        }
    };
    provider_registry::dispatch(request, &broker::provider_resource(provider), credential).await
}

/// Execute an ordered caller-independent route chain. A failed primary is
/// followed by each configured fallback; attempt accounting spans the chain.
pub async fn dispatch_direct_with_fallback(
    request: &ModelRequest,
    fallbacks: &[String],
) -> ModelResponse {
    let mut response = dispatch_direct(request).await;
    let mut attempts = response.attempts;
    for fallback in fallbacks {
        if response.success {
            break;
        }
        let mut next = request.clone();
        next.model = fallback.clone();
        response = dispatch_direct(&next).await;
        attempts = attempts.saturating_add(response.attempts);
    }
    response.attempts = attempts;
    response
}

pub async fn dispatch_direct_openai_typed(
    route_id: &str,
    path: &str,
    payload: serde_json::Map<String, Value>,
) -> Result<Value, String> {
    let provider =
        provider_for(route_id).ok_or_else(|| "unknown provider/model route".to_string())?;
    if provider_requires_caller_identity(route_id) {
        return Err("auth: caller identity is required for subscription providers".to_string());
    }
    let credential = broker::provider_credential(provider)
        .await
        .ok_or_else(|| format!("direct '{provider}' credential is unavailable"))?;
    let credential = credential
        .expose_utf8()
        .map_err(|_| format!("direct '{provider}' credential is not valid UTF-8"))?;
    provider_registry::dispatch_openai_typed(
        route_id,
        path,
        payload,
        &broker::provider_resource(provider),
        credential,
    )
    .await
}

/// Authenticate the Jeden caller, redeem the selected provider credential at
/// the final-use boundary, and execute one stateless provider API request.
pub async fn dispatch_subscription(
    headers: &HeaderMap,
    request: &ModelRequest,
    raw_body: &[u8],
) -> ModelResponse {
    let provider = match provider_for(&request.model) {
        Some(provider) => provider,
        None => {
            return ModelResponse::failure(&request.model, "unknown provider/model route".into())
        }
    };
    let agent_id = match authenticate_agent(headers, raw_body).await {
        Ok(agent_id) => agent_id,
        Err(error) => return ModelResponse::failure(&request.model, error),
    };
    dispatch_subscription_attempt(provider, &agent_id, request).await
}

async fn dispatch_subscription_attempt(
    provider: &str,
    agent_id: &str,
    request: &ModelRequest,
) -> ModelResponse {
    let rows = match eligible_subscription_entries(
        broker::list_subscriptions(agent_id).await,
        provider,
        request.billing_target.as_ref(),
    ) {
        Ok(rows) => rows,
        Err(error) => return ModelResponse::failure(&request.model, error),
    };
    if rows.is_empty() {
        return refuse(
            request,
            POINT_CREDENTIAL_SELECTION,
            request.billing_target.as_ref().map_or_else(
                || format!("no active '{provider}' credential for agent"),
                |target| {
                    format!(
                        "selected credential '{}' is not active for provider '{provider}' and agent",
                        target.subscription_id
                    )
                },
            ),
            None,
        );
    }

    let mut provider_attempts = u32::default();
    // Why this request was refused usually happened here, one layer down: a
    // refresh the provider rejected. Keeping it means the refusal can say so.
    let mut refresh_refusal: Option<Failure> = None;
    // Set when a provider refused a credential outright, as opposed to being out
    // of quota: the two empty the pool for different reasons and the caller has
    // to be told which.
    let mut saw_auth_rejection = false;
    for (index, entry) in rows.iter().take(max_credential_attempts()).enumerate() {
        let credential_id = &entry.id;
        // A credential inside a recorded block is skipped without a provider
        // call. The previous behaviour re-derived exhaustion from an error
        // string on every request, which meant paying for the 429 to learn what
        // the last 429 already said.
        if usage::is_blocked(credential_id) {
            warn!(
                event = "credential_blocked",
                provider,
                credential_index = index,
                "bounded credential is inside a recorded rate-limit block"
            );
            continue;
        }
        let token = match broker::subscription_credential(credential_id, provider).await {
            Some(token) => token,
            None => {
                warn!(
                    event = "credential_unavailable",
                    provider,
                    credential_index = index,
                    "bounded credential is unavailable"
                );
                continue;
            }
        };
        let token = match token.expose_utf8() {
            Ok(token) => token,
            Err(_) => {
                warn!(
                    event = "credential_invalid_encoding",
                    provider,
                    credential_index = index,
                    "bounded credential has invalid encoding"
                );
                continue;
            }
        };
        provider_attempts = provider_attempts.saturating_add(u32::from(true));
        let item = broker::subscription_resource(provider, credential_id);
        let mut result = provider_registry::dispatch(request, &item, token).await;
        result.attempts = provider_attempts;
        usage::record_call(credential_id, provider, &result);

        // Whether the provider refused a token it had just handed us. The
        // string matcher below cannot see that: the message is the same
        // "invalid authentication" either way, and only the sequence says the
        // credential is dead rather than stale.
        let mut rejected_with_fresh_token = false;
        if !result.success && result.error.as_deref().is_some_and(is_auth_failure) {
            match broker::refresh_subscription_credential(credential_id, provider).await {
                Ok(fresh) => {
                    if let Ok(fresh_token) = fresh.expose_utf8() {
                        provider_attempts = provider_attempts.saturating_add(u32::from(true));
                        result = provider_registry::dispatch(request, &item, fresh_token).await;
                        result.attempts = provider_attempts;
                        usage::record_call(credential_id, provider, &result);
                        if result.success {
                            info!(
                                event = "credential_refreshed",
                                provider,
                                credential_index = index,
                                "provider request succeeded after forced OAuth refresh"
                            );
                            return result;
                        }
                        rejected_with_fresh_token =
                            result.error.as_deref().is_some_and(is_auth_failure);
                    }
                }
                // The newest refusal is kept rather than the first: it is the
                // one that stopped this request, and the earlier credentials
                // logged theirs on the way past.
                Err(refused) => refresh_refusal = Some(refused),
            }
        }
        if result.success {
            if index > 0 {
                info!(
                    event = "credential_rotated",
                    provider,
                    credential_index = index,
                    "provider request succeeded after bounded rotation"
                );
            }
            return result;
        }
        let error = result.error.clone().unwrap_or_default();
        if is_permanent_auth_failure(&error) {
            warn!(
                event = "credential_retired",
                provider,
                credential_index = index,
                "provider permanently rejected bounded credential"
            );
            mark_credential_revoked(credential_id).await;
        }
        if rejected_with_fresh_token {
            // Retried forever otherwise: the refusal reads as an ordinary auth
            // failure, so the next request refreshes and is refused again. One
            // host wrote 275,468 of these lines against a single subscription
            // while every request paid for two provider round trips first.
            warn!(
                event = "credential_retired",
                provider,
                credential_index = index,
                "provider refused a token it had just issued; the subscription needs \
                 re-authorization and is retired until it is granted"
            );
            mark_credential_revoked(credential_id).await;
        }
        if is_auth_failure(&error) {
            saw_auth_rejection = true;
            warn!(
                event = "credential_auth_rejected",
                provider,
                credential_index = index,
                "provider rejected bounded credential after OAuth refresh"
            );
            continue;
        }
        let exhausted = error.contains("hit your limit")
            || error.contains("usage limit")
            || error.contains("rate_limit")
            || error.contains("429");
        if !exhausted {
            return result;
        }
        usage::record_block(credential_id, provider, &error, &result);
        warn!(
            event = "credential_exhausted",
            provider,
            credential_index = index,
            "provider rejected bounded credential with a rotatable failure"
        );
    }
    // Which cause emptied the pool decides what the caller is told. Every
    // exhausted credential is capacity and waiting helps; a provider that
    // rejected the credential is an authorization failure that waiting cannot
    // reach, and callers used to retry it as `429 capacity_error` while the
    // subscription sat burnt waiting for someone to log in.
    let summary = if saw_auth_rejection {
        format!(
            "all bounded '{provider}' credentials were rejected by the provider; \
             re-authorization required"
        )
    } else {
        format!("all bounded '{provider}' credentials unavailable for agent")
    };
    let mut failure = refuse(request, POINT_BOUNDED_ROTATION, summary, refresh_refusal);
    failure.attempts = provider_attempts;
    failure
}

fn string_field(row: &Value, key: &str) -> String {
    match row.get(key) {
        Some(Value::String(value)) => value.trim().to_string(),
        Some(Value::Number(value)) => value.to_string(),
        Some(Value::Bool(value)) => value.to_string(),
        _ => String::new(),
    }
}
