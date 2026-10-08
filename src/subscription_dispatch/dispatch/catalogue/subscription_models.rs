//! Which models an agent's own subscriptions publish, asked of each provider
//! once and remembered for as long as the answer stands.

use std::collections::HashMap;
use std::time::Instant;
use tracing::{info, warn};

use crate::gateway::broker;
use crate::providers::adapter as provider_registry;
use crate::subscription_dispatch::usage;
use crate::types::{GatewayRefusal, ProviderRefusal, Refusal};

use super::super::refusal::envelope::{credential_refusal_class, failure_detail};
use super::cache::{
    cached_subscription_models, lock_discovery, CachedRegistryModels, MODEL_FAILURE_CACHE_TTL,
    REGISTRY_MODEL_CACHE, REGISTRY_MODEL_FAILURE_CACHE,
};

pub async fn registry_models_for_agent(
    agent_id: &str,
) -> Result<Vec<provider_registry::RegistryModel>, Refusal> {
    // The list comes from the vault's broker; its cost is logged beside the
    // discovery's own, so a selector request whose minutes go before any
    // provider is asked shows which of the two spent them.
    let listing = Instant::now();
    let listed = broker::list_subscriptions(agent_id).await;
    info!(
        event = "subscriptions_listed",
        agent_id,
        subscriptions = listed.len(),
        list_ms = listing.elapsed().as_millis(),
        "an agent's subscriptions listed by the broker"
    );
    let entries = listed
        .into_iter()
        .filter(|entry| entry.status == "active" && !crate::journal::is_retired(&entry.id))
        .collect::<Vec<_>>();
    discover_subscription_models(entries).await
}

pub(super) async fn discover_subscription_models(
    entries: Vec<broker::SubscriptionEntry>,
) -> Result<Vec<provider_registry::RegistryModel>, Refusal> {
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
        let cached = cached_subscription_models(&cache_key);
        let _discovery = if cached.is_none() {
            Some(lock_discovery(&cache_key).await)
        } else {
            None
        };
        let cached = cached.or_else(|| cached_subscription_models(&cache_key));
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
                failures.push(Refusal::gateway(
                    GatewayRefusal::SubscriptionReauthorizationRequired,
                    format!("{}: awaiting sign-in: {cause}", entry.id),
                ));
                continue;
            }
            let recent_failure = REGISTRY_MODEL_FAILURE_CACHE.lock().ok().and_then(|cache| {
                cache
                    .get(&cache_key)
                    .filter(|(fetched, _)| fetched.elapsed() < MODEL_FAILURE_CACHE_TTL)
                    .map(|(_, refused)| refused.clone())
            });
            if let Some(refused) = recent_failure {
                failures.push(Refusal::new(
                    refused.class,
                    format!("{}: {}", entry.id, refused.message),
                ));
                continue;
            }
            read += 1;
            let secret = match broker::subscription_credential(&entry.id, provider).await {
                Ok(secret) => secret,
                Err(refused) => {
                    let detail = failure_detail(&refused);
                    let class = credential_refusal_class(&refused);
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
                        cache.insert(
                            cache_key.clone(),
                            (Instant::now(), Refusal::gateway(class, detail.clone())),
                        );
                    }
                    failures.push(Refusal::gateway(class, format!("{}: {detail}", entry.id)));
                    continue;
                }
            };
            let secret = match secret.expose_utf8() {
                Ok(secret) => secret,
                Err(error) => {
                    failures.push(Refusal::gateway(
                        GatewayRefusal::ProviderFailure,
                        format!("{}: credential is not UTF-8: {error}", entry.id),
                    ));
                    continue;
                }
            };
            let item = broker::subscription_resource(provider, &entry.id);
            let discovered = match provider_registry::discover_models(provider, &item, secret).await
            {
                Ok(models) => models,
                Err(error) => {
                    // The provider's model list could not be read.
                    let refused = Refusal::gateway(GatewayRefusal::DependencyUnavailable, error);
                    if let Ok(mut cache) = REGISTRY_MODEL_FAILURE_CACHE.lock() {
                        cache.insert(cache_key.clone(), (Instant::now(), refused.clone()));
                    }
                    failures.push(Refusal::new(
                        refused.class,
                        format!("{}: {}", entry.id, refused.message),
                    ));
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
        return Err(Refusal::new(
            discovery_class(&failures),
            format!(
                "could not discover native provider models: {}",
                failures
                    .iter()
                    .map(|refused| refused.message.as_str())
                    .collect::<Vec<_>>()
                    .join("; ")
            ),
        ));
    }
    let mut models = models_by_route.into_values().collect::<Vec<_>>();
    models.sort_by(|left, right| left.route_id.cmp(&right.route_id));
    Ok(models)
}

/// The class a discovery that found nothing is answered with: a dependency
/// that did not answer first, because a retry may reach it; then the
/// authorization failures only an operator repairs.
fn discovery_class(failures: &[Refusal]) -> ProviderRefusal {
    [
        ProviderRefusal::Gateway(GatewayRefusal::DependencyUnavailable),
        ProviderRefusal::Gateway(GatewayRefusal::CredentialUnauthorized),
        ProviderRefusal::Gateway(GatewayRefusal::SubscriptionReauthorizationRequired),
    ]
    .into_iter()
    .find(|class| failures.iter().any(|refused| refused.class == *class))
    .unwrap_or(ProviderRefusal::Gateway(GatewayRefusal::ProviderFailure))
}
