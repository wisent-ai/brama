//! The live listing and the cache in front of it: one shell read of the pool,
//! stored only after it succeeded so a failed read is never what a later caller
//! sees, and each row completed with the account it was recorded against.

use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use tracing::warn;

use super::super::super::vault::{entitlements_router_bin, existing_item_account, raw_listing};
use super::super::account::{parse_live_subscriptions, parse_owned_subscriptions, SubscriptionEntry};
use super::super::donation::donated_subscriptions;

type LiveSubscriptionsCache = Mutex<Option<(Instant, Vec<SubscriptionEntry>)>>;

/// The live pool listing. Stored only after a successful listing so a failed
/// shell never poisons the cache.
static LIVE_SUBSCRIPTIONS_CACHE: LazyLock<LiveSubscriptionsCache> =
    LazyLock::new(|| Mutex::new(None));

const LIVE_SUBSCRIPTIONS_CACHE_TTL: Duration = Duration::from_secs(60);

/// Resolve the pool from the vault, serving a fresh cached listing unless
/// `bypass_cache` is set (used when a lookup failed and the caller wants to
/// re-check the vault instead of trusting a stale entry).
pub(super) async fn live_subscriptions(
    broker: &str,
    bypass_cache: bool,
) -> Result<Vec<SubscriptionEntry>, String> {
    if !bypass_cache {
        if let Ok(cache) = LIVE_SUBSCRIPTIONS_CACHE.lock() {
            if let Some((fetched_at, entries)) = cache.as_ref() {
                if fetched_at.elapsed() < LIVE_SUBSCRIPTIONS_CACHE_TTL {
                    return Ok(entries.clone());
                }
            }
        }
    }
    let entries = list_subscriptions_live(broker).await?;
    if let Ok(mut cache) = LIVE_SUBSCRIPTIONS_CACHE.lock() {
        *cache = Some((Instant::now(), entries.clone()));
    }
    Ok(entries)
}

/// Complete each member's account from its own vault item where its tags do
/// not carry one.
///
/// The tag is the cheap answer and the item's own `account_ref` is the
/// authoritative one; a vault older than the `brama:account:` namespace can
/// only hold the latter, and a member whose account was recorded before the
/// namespace existed carries it there alone. It costs one item read per
/// member that has no account tag, and nothing of the credential is kept:
/// only the address, which is what the pool counts accounts by.
///
/// Every audience goes through this, because an operator asking how many
/// accounts this deployment holds must not get a different answer from the
/// console, the pool document and the router.
pub(super) async fn with_recorded_accounts(mut entries: Vec<SubscriptionEntry>) -> Vec<SubscriptionEntry> {
    for entry in &mut entries {
        if entry.account.is_some() {
            continue;
        }
        let item = format!(
            "provider:{}:{}",
            super::super::slug(&entry.provider),
            super::super::slug(&entry.id)
        );
        if let Ok(recorded) = existing_item_account(&item).await {
            entry.account = recorded;
        }
    }
    entries
}

/// Shell the entitlements router's bare `list`, which returns a JSON array of
/// every vault item (`{"id","type","tags","updated_at","deleted","versions"}`),
/// with each member's recorded account completed.
pub(super) async fn list_subscriptions_live(broker: &str) -> Result<Vec<SubscriptionEntry>, String> {
    let stdout = raw_listing(broker, "list subscriptions").await?;
    Ok(with_recorded_accounts(parse_live_subscriptions(&stdout)?).await)
}

pub(super) async fn list_subscriptions_result(agent_id: &str) -> Result<Vec<SubscriptionEntry>, String> {
    let broker = entitlements_router_bin();
    match live_subscriptions(&broker, false).await {
        Ok(mut live) => {
            match configured_subscriptions() {
                Some(Ok(configured)) => {
                    for entry in configured {
                        if !live.iter().any(|existing| existing.id == entry.id) {
                            live.push(entry);
                        }
                    }
                }
                Some(Err(error)) => warn!(
                    event = "subscription_catalog_invalid",
                    agent_id,
                    %error,
                    "live subscriptions remain usable without the invalid trusted catalog"
                ),
                None => {}
            }
            Ok(live)
        }
        Err(live_error) => {
            warn!(
                event = "subscription_live_discovery_failed",
                agent_id,
                error = %live_error,
                "internal routing will use the trusted catalog if one is available"
            );
            match configured_subscriptions() {
                Some(Ok(configured)) => Ok(configured),
                Some(Err(catalog_error)) => Err(format!(
                    "{live_error}; trusted subscription catalog is invalid: {catalog_error}"
                )),
                None => Err(live_error),
            }
        }
    }
}
