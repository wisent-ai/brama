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
    cached_discovery_refusal_since, cached_subscription_models_since, lock_discovery,
    CachedRegistryModels, REGISTRY_MODEL_CACHE, REGISTRY_MODEL_FAILURE_CACHE,
};

pub async fn registry_models_for_agent(
    agent_id: &str,
) -> Result<Vec<provider_registry::RegistryModel>, Refusal> {
    // Routing reads observed inventory. A vault refresh runs outside the
    // request, and its pending or failed state is a refusal, not a held call.
    let listing = Instant::now();
    let listed = broker::routing_subscriptions(agent_id)
        .map_err(|error| Refusal::gateway(GatewayRefusal::DependencyUnavailable, error))?;
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
    super::cache::routing::models(entries)
}

/// A standalone diagnostic explicitly discovers before dispatch; service
/// requests instead read the background snapshot and never join that work.
pub async fn prepare_routing(agent_id: &str, discover_models: bool) -> Result<(), Refusal> {
    let entries = broker::refresh_routing_subscriptions(agent_id)
        .await
        .map_err(|error| Refusal::gateway(GatewayRefusal::DependencyUnavailable, error))?;
    if !discover_models {
        return Ok(());
    }
    let entries = entries
        .into_iter()
        .filter(|entry| entry.status == "active" && !crate::journal::is_retired(&entry.id))
        .collect();
    super::cache::routing::refresh(entries).await.map(|_| ())
}

pub(super) async fn discover_subscription_models(
    entries: Vec<broker::SubscriptionEntry>,
) -> Result<Vec<provider_registry::RegistryModel>, Refusal> {
    if entries.is_empty() {
        return Ok(Vec::new());
    }

    let started = Instant::now();
    let subscriptions = entries.len();
    let mut refused_by_ledger = 0usize;
    let mut read = 0usize;
    let mut models_by_route = HashMap::new();
    let mut failures = Vec::new();
    for entry in entries {
        let provider = entry.provider.trim();
        let cache_key = format!("{provider}:{}", entry.id);
        // The ledger's verdict comes before anything else, the cache
        // included. A grant the provider disowned, a member an operator
        // retired, or a credential inside a recorded block is not presented
        // by the rotation walk, so reading its models would only rank routes
        // the walk then skips; and the read is not free: a child process, a
        // broker redemption and a provider round trip, paid by the caller
        // whose request ranked this subscription. The walk reads the same
        // verdict, so a pool the ledger refuses entirely is refused here,
        // before any vault child or provider is asked.
        if let Some(refused) = usage::standing_refusal(&entry.id) {
            refused_by_ledger += 1;
            failures.push(Refusal::new(
                refused.class,
                format!("{}: {}", entry.id, refused.message),
            ));
            continue;
        }
        let cached = cached_subscription_models_since(&cache_key, started);
        let _discovery = if cached.is_none() {
            Some(lock_discovery(&cache_key).await)
        } else {
            None
        };
        // The holder of the lock read for everyone queued behind it: a
        // reading or a refusal produced after this caller began is its answer,
        // declared age or none.
        let cached = cached.or_else(|| cached_subscription_models_since(&cache_key, started));
        let models = if let Some(cached) = cached {
            cached
        } else {
            if let Some(refused) = cached_discovery_refusal_since(&cache_key, started) {
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
    // many the ledger refused before any read, and what it all cost.
    info!(
        event = "subscription_models_discovered",
        subscriptions,
        read,
        refused_by_ledger,
        failed = failures.len(),
        elapsed_ms = started.elapsed().as_millis() as u64,
        "discovered the models an agent's subscriptions publish"
    );
    if models_by_route.is_empty() {
        if let Some(first) = failures.first() {
            let refused = failures
                .iter()
                .map(|refused| refused.message.as_str())
                .collect::<Vec<_>>()
                .join("; ");
            // A pool the ledger refused entirely was refused before any vault
            // child or provider was asked; the sentence says so, because
            // "could not discover" would send the reader to look for a read
            // that never happened.
            let message = if refused_by_ledger == subscriptions {
                format!("every subscription is refused by the ledger before any is read: {refused}")
            } else {
                format!("could not discover native provider models: {refused}")
            };
            return Err(Refusal::new(discovery_class(first, &failures), message));
        }
    }
    let mut models = models_by_route.into_values().collect::<Vec<_>>();
    models.sort_by(|left, right| left.route_id.cmp(&right.route_id));
    Ok(models)
}

/// The class a discovery that found nothing is answered with: a dependency
/// that did not answer first, because a retry may reach it; then what the
/// ledger's blocks say in the rotation walk's own order — a spent paid
/// balance, which no wait repairs, then a rate window, which one does; then
/// the authorization failures only an operator repairs. Every refusal carries
/// the class it was made with, so when none of those is among them the first
/// refusal answers with its own.
fn discovery_class(first: &Refusal, failures: &[Refusal]) -> ProviderRefusal {
    let telling = [
        ProviderRefusal::Gateway(GatewayRefusal::DependencyUnavailable),
        ProviderRefusal::QuotaExhausted,
        ProviderRefusal::RateLimited,
        ProviderRefusal::Gateway(GatewayRefusal::CredentialUnauthorized),
        ProviderRefusal::Gateway(GatewayRefusal::SubscriptionReauthorizationRequired),
    ]
    .into_iter()
    .find(|class| failures.iter().any(|refused| refused.class == *class));
    match telling {
        Some(class) => class,
        None => first.class,
    }
}
