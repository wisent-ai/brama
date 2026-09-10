//! Which models an agent's own subscriptions publish, asked of each provider
//! once and remembered for as long as the answer stands.

use std::collections::HashMap;
use std::time::Instant;
use tracing::{info, warn};

use crate::gateway::broker;
use crate::providers::adapter as provider_registry;
use crate::subscription_dispatch::usage;

use super::super::refusal::envelope::failure_detail;
use super::cache::{
    CachedRegistryModels, MODEL_CACHE_TTL, MODEL_FAILURE_CACHE_TTL, REGISTRY_MODEL_CACHE,
    REGISTRY_MODEL_FAILURE_CACHE,
};

pub async fn registry_models_for_agent(
    agent_id: &str,
) -> Result<Vec<provider_registry::RegistryModel>, String> {
    let entries = broker::list_subscriptions(agent_id)
        .await
        .into_iter()
        .filter(|entry| entry.status == "active" && !crate::journal::is_retired(&entry.id))
        .collect::<Vec<_>>();
    discover_subscription_models(entries).await
}

pub(super) async fn discover_subscription_models(
    entries: Vec<broker::SubscriptionEntry>,
) -> Result<Vec<provider_registry::RegistryModel>, String> {
    if entries.is_empty() {
        return Ok(Vec::new());
    }

    let started = Instant::now();
    let subscriptions = entries.len();
    let mut awaiting_sign_in = 0usize;
    let mut read = 0usize;
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
            // The ledger's verdict comes before anything else. A grant the
            // provider disowned is refused again on every read, and the read
            // is not free: a child process, a broker redemption and a provider
            // round trip for the refresh, paid by the caller whose request
            // ranked this subscription. The sweep already leaves such a grant
            // alone until a sign-in replaces it; so does this, and it says so
            // rather than reporting the sixty-second memo of the last refusal.
            if let Some(cause) = usage::awaiting_sign_in_cause(&entry.id) {
                awaiting_sign_in += 1;
                failures.push(format!("{}: awaiting sign-in: {cause}", entry.id));
                continue;
            }
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
            read += 1;
            let secret = match broker::subscription_credential(&entry.id, provider).await {
                Ok(secret) => secret,
                Err(refused) => {
                    let detail = failure_detail(&refused);
                    warn!(
                        event = "subscription_model_credential_failed",
                        subscription = %entry.id,
                        provider,
                        envelope = %refused.to_json(),
                        "{}",
                        refused.render()
                    );
                    // Remembered like a discovery failure, for the same reason:
                    // the next request is not a new fact about this credential.
                    if let Ok(mut cache) = REGISTRY_MODEL_FAILURE_CACHE.lock() {
                        cache.insert(cache_key.clone(), (Instant::now(), detail.clone()));
                    }
                    failures.push(format!("{}: {detail}", entry.id));
                    continue;
                }
            };
            let secret = match secret.expose_utf8() {
                Ok(secret) => secret,
                Err(error) => {
                    failures.push(format!("{}: credential is not UTF-8: {error}", entry.id));
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
    // One line per discovery, so a slow request can be attributed: how many
    // subscriptions were answered from the cache, how many from the vault, how
    // many were left alone because a sign-in is owed, and what it all cost.
    info!(
        event = "subscription_models_discovered",
        subscriptions,
        read,
        awaiting_sign_in,
        failed = failures.len(),
        elapsed_ms = started.elapsed().as_millis() as u64,
        "discovered the models an agent's subscriptions publish"
    );
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
