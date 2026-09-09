//! What discovery has already learned about a credential's models, how long
//! that answer stands, and why it refused when it did.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use crate::providers::adapter as provider_registry;

pub(super) struct CachedRegistryModels {
    pub(super) fetched: Instant,
    pub(super) models: Vec<provider_registry::RegistryModel>,
}

pub(super) const MODEL_CACHE_TTL: Duration = Duration::from_secs(300);

pub(super) static REGISTRY_MODEL_CACHE: LazyLock<Mutex<HashMap<String, CachedRegistryModels>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Discovery failures are cached briefly too: without it every catalog call
/// re-pays full provider timeouts for credentials that are stale anyway.
pub(super) const MODEL_FAILURE_CACHE_TTL: Duration = Duration::from_secs(60);
pub(super) static REGISTRY_MODEL_FAILURE_CACHE: LazyLock<
    Mutex<HashMap<String, (Instant, String)>>,
> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// Why model discovery last refused one subscription, when it did.
///
/// [`discover_subscription_models`](super::subscription_models::discover_subscription_models)
/// records every per-subscription refusal and then throws the list away unless
/// the pool ended up with no models at all — so one working subscription
/// silences the reason every other one failed, and `/readyz` could say only
/// "active subscription, no model discovered".
///
/// Measured on charless-mac-mini on 2026-09-02: claude-code and kimi both
/// redeemed and both discovered nothing, while codex on the same host in the
/// same sweep discovered five. Establishing why took reading this module's
/// branches and counting `static_models` entries, because no surface would say
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
        .filter(|(fetched, _)| fetched.elapsed() < MODEL_FAILURE_CACHE_TTL)
        .map(|(_, error)| error.clone())
}

/// Everything currently held in the discovery cache, whatever put it there.
pub(super) fn cached_registry_models() -> Vec<provider_registry::RegistryModel> {
    let Ok(cache) = REGISTRY_MODEL_CACHE.lock() else {
        return Vec::new();
    };
    cache
        .values()
        .filter(|item| item.fetched.elapsed() < MODEL_CACHE_TTL)
        .flat_map(|item| item.models.clone())
        .collect()
}
