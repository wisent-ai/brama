//! The catalogue the desktop console reads: everything already discovered for
//! this deployment, refreshed behind the answer rather than inside it.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use crate::gateway::broker;
use crate::providers::adapter as provider_registry;

use super::cache::{
    cached_registry_models, CachedRegistryModels, MODEL_CACHE_TTL, MODEL_FAILURE_CACHE_TTL,
    REGISTRY_MODEL_CACHE, REGISTRY_MODEL_FAILURE_CACHE,
};
use super::subscription_models::discover_subscription_models;

/// Models already discovered for this deployment's subscriptions and its
/// directly-keyed providers, for the desktop console.
///
/// The console authenticates with a bearer, not an agent signature, so it has
/// no agent identity to discover against. Without this its catalogue carried
/// the public vendor list alone -- which knows `openai` but not that a `codex`
/// subscription is what pays for those models here -- so a subscription's own
/// screen could not name a single model it pays for.
///
/// This reads caches and never waits on a provider. Discovering ten providers
/// inline, several holding credentials their vendor has since revoked, took
/// longer than the console's own request deadline: the catalogue answered with
/// a timeout, which is worse than answering with the part that is known. A cold
/// cache is filled by one background pass instead, so the first read is fast
/// and thin and the next is complete.
pub async fn registry_models_for_console() -> Result<Vec<provider_registry::RegistryModel>, String>
{
    let mut models = cached_registry_models();
    spawn_console_discovery();
    models.sort_by(|left, right| left.route_id.cmp(&right.route_id));
    models.dedup_by(|left, right| left.route_id == right.route_id);
    Ok(models)
}

/// One background discovery pass at a time. A console that refreshes every few
/// seconds must not stack a provider sweep per click.
static CONSOLE_DISCOVERY_RUNNING: AtomicBool = AtomicBool::new(false);

fn spawn_console_discovery() {
    if CONSOLE_DISCOVERY_RUNNING.swap(true, Ordering::SeqCst) {
        return;
    }
    tokio::spawn(async move {
        let entries = match broker::list_all_subscriptions().await {
            Ok(entries) => entries
                .into_iter()
                .filter(|entry| entry.status == "active" && !crate::journal::is_retired(&entry.id))
                .collect::<Vec<_>>(),
            Err(error) => {
                tracing::warn!(
                    %error,
                    event = "console_subscription_listing_failed",
                    "subscription models could not be refreshed for the console catalogue"
                );
                Vec::new()
            }
        };
        if let Err(error) = discover_subscription_models(entries).await {
            tracing::warn!(
                %error,
                event = "console_subscription_discovery_failed",
                "no subscription published models for the console catalogue"
            );
        }
        discover_direct_provider_models().await;
        CONSOLE_DISCOVERY_RUNNING.store(false, Ordering::SeqCst);
    });
}

/// Models for the providers this gateway holds a direct credential for.
///
/// A subscription is not the only way a provider gets paid for, and the console
/// showed the consequence: `featherless` sat there marked available with a model
/// count of zero, because nothing discovered a provider unless a subscription
/// pointed at it and the public vendor list has no `featherless` in it at all.
///
/// A provider that cannot be reached contributes nothing and says so in the
/// log; it does not fail the catalogue for the others.
async fn discover_direct_provider_models() -> Vec<provider_registry::RegistryModel> {
    let mut discovered = Vec::new();
    for provider in broker::configured_provider_capabilities() {
        let cache_key = format!("{provider}:direct");
        if let Some(models) = REGISTRY_MODEL_CACHE.lock().ok().and_then(|cache| {
            cache
                .get(&cache_key)
                .filter(|item| item.fetched.elapsed() < MODEL_CACHE_TTL)
                .map(|item| item.models.clone())
        }) {
            discovered.extend(models);
            continue;
        }
        if REGISTRY_MODEL_FAILURE_CACHE
            .lock()
            .ok()
            .and_then(|cache| {
                cache
                    .get(&cache_key)
                    .filter(|(fetched, _)| fetched.elapsed() < MODEL_FAILURE_CACHE_TTL)
                    .cloned()
            })
            .is_some()
        {
            continue;
        }
        let Some(secret) = broker::provider_credential(&provider).await else {
            continue;
        };
        let Ok(secret) = secret.expose_utf8() else {
            continue;
        };
        let resource = broker::provider_resource(&provider);
        match provider_registry::discover_models(&provider, &resource, secret).await {
            Ok(models) => {
                if let Ok(mut cache) = REGISTRY_MODEL_CACHE.lock() {
                    cache.insert(
                        cache_key,
                        CachedRegistryModels {
                            fetched: Instant::now(),
                            models: models.clone(),
                        },
                    );
                }
                discovered.extend(models);
            }
            Err(error) => {
                if let Ok(mut cache) = REGISTRY_MODEL_FAILURE_CACHE.lock() {
                    cache.insert(cache_key, (Instant::now(), error.clone()));
                }
                tracing::warn!(
                    %provider,
                    %error,
                    event = "direct_provider_discovery_failed",
                    "a provider with a direct credential published no models"
                );
            }
        }
    }
    discovered
}
