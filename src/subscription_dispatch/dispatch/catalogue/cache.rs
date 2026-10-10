//! What discovery has already learned about a credential's models, how long
//! the operator declared that answer stands, and why it refused when it did.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Instant;

use crate::providers::adapter as provider_registry;
use crate::types::Refusal;

pub(super) struct CachedRegistryModels {
    pub(super) fetched: Instant,
    pub(super) models: Vec<provider_registry::RegistryModel>,
}

/// The age, in whole seconds, at which a credential's discovered models are
/// read again. Unset, every catalog call discovers: no age is assumed.
const MODEL_AGE_ENV: &str = "BRAMA_MODEL_DISCOVERY_AGE_SECONDS";

/// Whether models discovered at `fetched` may still be served.
pub(super) fn models_fresh(fetched: Instant) -> bool {
    crate::types::still_fresh(MODEL_AGE_ENV, fetched)
}

pub(super) static REGISTRY_MODEL_CACHE: LazyLock<Mutex<HashMap<String, CachedRegistryModels>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// A discovery refusal may be held too, so a catalog call does not wait on a
/// provider for a credential that is stale anyway — for the age the operator
/// declares, and not at all when none is. The refusal keeps its class, so a
/// request answered from the cache is answered as the first one was.
const FAILURE_AGE_ENV: &str = "BRAMA_MODEL_DISCOVERY_FAILURE_AGE_SECONDS";

/// Whether a discovery refusal recorded at `fetched` may still be served.
pub(super) fn failure_fresh(fetched: Instant) -> bool {
    crate::types::still_fresh(FAILURE_AGE_ENV, fetched)
}

pub(super) static REGISTRY_MODEL_FAILURE_CACHE: LazyLock<
    Mutex<HashMap<String, (Instant, Refusal)>>,
> = LazyLock::new(|| Mutex::new(HashMap::new()));

type DiscoveryLocks = Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>;
static DISCOVERY_LOCKS: LazyLock<DiscoveryLocks> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// The models discovered for `key` that a caller who began at `since` may
/// take: a reading within the declared age, or one produced after the caller
/// began waiting. The second is what coalescing means — the holder of the
/// discovery lock read for everyone queued behind it, and handing those
/// callers an answer newer than their own request assumes no age at all.
/// Without it, an undeclared age sent every queued caller to the vault and
/// the provider in turn, and a burst of requests became a queue of hours.
pub(super) fn cached_subscription_models_since(
    key: &str,
    since: Instant,
) -> Option<Vec<provider_registry::RegistryModel>> {
    REGISTRY_MODEL_CACHE.lock().ok().and_then(|cache| {
        cache
            .get(key)
            .filter(|item| models_fresh(item.fetched) || item.fetched >= since)
            .map(|item| item.models.clone())
    })
}

/// The refusal discovery recorded for `key` that a caller who began at `since`
/// may take, on the same terms as [`cached_subscription_models_since`].
pub(super) fn cached_discovery_refusal_since(key: &str, since: Instant) -> Option<Refusal> {
    REGISTRY_MODEL_FAILURE_CACHE.lock().ok().and_then(|cache| {
        cache
            .get(key)
            .filter(|(fetched, _)| failure_fresh(*fetched) || *fetched >= since)
            .map(|(_, refused)| refused.clone())
    })
}

/// Coalesce a cold read by subscription, never across unrelated accounts.
/// The caller checks both caches again after acquiring this guard. Only model
/// metadata and refusals are shared; inference still redeems its own credential.
pub(super) async fn lock_discovery(key: &str) -> tokio::sync::OwnedMutexGuard<()> {
    let lock = {
        let mut locks = DISCOVERY_LOCKS
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(lock) = locks.get(key) {
            Arc::clone(lock)
        } else {
            let lock = Arc::new(tokio::sync::Mutex::new(()));
            locks.insert(key.to_owned(), Arc::clone(&lock));
            lock
        }
    };
    lock.lock_owned().await
}

/// Why model discovery last refused one subscription, when it did.
///
/// [`discover_subscription_models`](super::subscription_models::discover_subscription_models)
/// records every per-subscription refusal and then throws the list away unless
/// the pool ended up with no models at all — so one working subscription
/// silences the reason every other one failed, and `/readyz` could say only
/// "active subscription, no model discovered".
///
/// Two providers can redeem and discover nothing while a third on the same
/// host in the same sweep discovers models. Establishing why would take
/// reading this module's branches and counting `static_models` entries,
/// because no surface would say
/// it. Two of the three explanations that reading had to separate — a
/// catalogue with nothing configured for the provider, and a discovery path
/// that never ran — are ours to fix, and the third is not; the sentence this
/// exposes names which.
pub fn discovery_failure(provider: &str, subscription_id: &str) -> Option<String> {
    let key = format!("{}:{subscription_id}", provider.trim());
    REGISTRY_MODEL_FAILURE_CACHE
        .lock()
        .ok()?
        .get(&key)
        .filter(|(fetched, _)| failure_fresh(*fetched))
        .map(|(_, refused)| refused.message.clone())
}

/// Everything currently held in the discovery cache, whatever put it there.
pub(super) fn cached_registry_models() -> Vec<provider_registry::RegistryModel> {
    let Ok(cache) = REGISTRY_MODEL_CACHE.lock() else {
        return Vec::new();
    };
    cache
        .values()
        .filter(|item| models_fresh(item.fetched))
        .flat_map(|item| item.models.clone())
        .collect()
}
