//! Which models an agent's own subscriptions publish, asked of each provider
//! once and remembered for as long as the answer stands.

use std::collections::HashMap;
use std::time::Instant;
use tracing::warn;

use crate::gateway::broker;
use crate::providers::adapter as provider_registry;

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
