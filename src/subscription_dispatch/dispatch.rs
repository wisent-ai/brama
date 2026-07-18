//! Route canonical `provider/model` requests through stateless provider APIs.

use aes_gcm::aead::rand_core::{OsRng, RngCore};
use axum::http::HeaderMap;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use crate::crypto;
use crate::gateway::broker;
use crate::subscription_dispatch::provider_registry;
use crate::types::{ModelRequest, ModelResponse};
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
        || error.contains("oauth")
        || is_permanent_auth_failure(&error)
}

async fn mark_credential_revoked(credential_id: &str) {
    crate::journal::retire(credential_id);
}

struct CachedRegistryModels {
    fetched: Instant,
    models: Vec<crate::subscription_dispatch::provider_registry::RegistryModel>,
}

static REGISTRY_MODEL_CACHE: LazyLock<Mutex<HashMap<String, CachedRegistryModels>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

pub fn is_subscription_model(model: &str) -> bool {
    provider_for(model).is_some()
}

pub(crate) fn provider_for(model: &str) -> Option<&str> {
    provider_registry::provider_id_from_route(model)
}

pub async fn registry_models_for_agent(
    agent_id: &str,
) -> Result<Vec<crate::subscription_dispatch::provider_registry::RegistryModel>, String> {
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
            let discovered = match crate::subscription_dispatch::provider_registry::discover_models(
                provider, secret,
            )
            .await
            {
                Ok(models) => models,
                Err(error) => {
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

fn shuffle_models(models: &mut [String]) {
    if models.len() < 2 {
        return;
    }
    for i in (1..models.len()).rev() {
        let j = (OsRng.next_u64() as usize) % (i + 1);
        models.swap(i, j);
    }
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
    shuffle_models(&mut models);
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
    shuffle_models(&mut models);
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
        shuffle_models(&mut group);
        ordered.extend(group);
    }
    Ok(ordered)
}

/// `model: "any"` selects among active stateless provider routes for the
/// signed agent and rotates across credentials on provider exhaustion.
pub async fn dispatch_any_subscription(
    headers: &HeaderMap,
    request: &ModelRequest,
    raw_body: &[u8],
) -> ModelResponse {
    let models = match any_subscription_models(headers, raw_body).await {
        Ok(models) => models,
        Err(e) => return ModelResponse::failure(&request.model, e),
    };
    let mut errors = Vec::new();
    for model in models {
        let mut candidate = request.clone();
        candidate.model = model.clone();
        let resp = dispatch_subscription(headers, &candidate, raw_body).await;
        if resp.success {
            return resp;
        }
        errors.push(format!(
            "{}: {}",
            model,
            resp.error.unwrap_or_else(|| "failed".to_string())
        ));
    }
    ModelResponse::failure(
        &request.model,
        format!(
            "no working subscription model for signed agent; tried {}",
            errors.join("; ")
        ),
    )
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
    let mut errors = Vec::new();
    for model in models {
        let mut candidate = request.clone();
        candidate.model = model.clone();
        let resp = dispatch_subscription(headers, &candidate, raw_body).await;
        if resp.success {
            return resp;
        }
        errors.push(format!(
            "{}: {}",
            model,
            resp.error.unwrap_or_else(|| "failed".to_string())
        ));
    }
    ModelResponse::failure(
        &request.model,
        format!(
            "no working vision-capable subscription model for signed agent; tried {}",
            errors.join("; ")
        ),
    )
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
    let mut errors = Vec::new();
    for model in models {
        let mut candidate = request.clone();
        candidate.model = model.clone();
        let resp = dispatch_subscription(headers, &candidate, raw_body).await;
        if resp.success {
            return resp;
        }
        errors.push(format!(
            "{}: {}",
            model,
            resp.error.unwrap_or_else(|| "failed".to_string())
        ));
    }
    ModelResponse::failure(
        &request.model,
        format!(
            "no working quality-ranked model for task '{task}'; tried {}",
            errors.join("; ")
        ),
    )
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
        return ModelResponse::failure(
            &request.model,
            request.billing_target.as_ref().map_or_else(
                || format!("no active '{provider}' credential for agent"),
                |target| {
                    format!(
                        "selected credential '{}' is not active for provider '{provider}' and agent",
                        target.subscription_id
                    )
                },
            ),
        );
    }

    let mut last_failure = None;
    for (index, entry) in rows.iter().enumerate() {
        let credential_id = &entry.id;
        let token = match broker::subscription_credential(credential_id, provider).await {
            Some(token) => token,
            None => {
                eprintln!(
                    "[router] credential {credential_id} is unavailable for provider {provider}"
                );
                continue;
            }
        };
        let token = match token.expose_utf8() {
            Ok(token) => token,
            Err(_) => {
                eprintln!(
                    "[router] credential {credential_id} is not valid UTF-8 for provider {provider}"
                );
                continue;
            }
        };
        let result = provider_registry::dispatch(request, token).await;
        if result.success {
            if index > 0 {
                eprintln!(
                    "[router] rotated to credential {credential_id} (idx={index}) for {provider}"
                );
            }
            return result;
        }
        let error = result.error.clone().unwrap_or_default();
        if is_permanent_auth_failure(&error) {
            eprintln!("[router] credential {credential_id} permanently rejected; retiring");
            mark_credential_revoked(credential_id).await;
        }
        let exhausted = is_auth_failure(&error)
            || error.contains("hit your limit")
            || error.contains("usage limit")
            || error.contains("rate_limit")
            || error.contains("429");
        if !exhausted {
            return result;
        }
        eprintln!(
            "[router] credential {credential_id} exhausted for {provider} ({error}); rotating"
        );
        last_failure = Some(result);
    }
    last_failure.unwrap_or_else(|| {
        ModelResponse::failure(
            &request.model,
            format!("all '{provider}' credentials unavailable for agent"),
        )
    })
}

fn string_field(row: &Value, key: &str) -> String {
    match row.get(key) {
        Some(Value::String(value)) => value.trim().to_string(),
        Some(Value::Number(value)) => value.to_string(),
        Some(Value::Bool(value)) => value.to_string(),
        _ => String::new(),
    }
}
