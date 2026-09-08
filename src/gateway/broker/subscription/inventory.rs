//! Which subscriptions exist, for one agent and for this deployment.
//!
//! Discovery is one question with several audiences, and the difference
//! between them is the whole subject: a user-facing import must not present an
//! unavailable Skarbiec as an empty account, internal routing may keep going
//! on the trusted catalog, the console wants every agent's rows, and renewal
//! wants the accounts the listing itself has lost. The per-agent cache lives
//! here too, because a listing is the only thing worth caching -- it is read
//! per request and a failed shell must never be what a later caller sees.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use tracing::warn;

use super::super::vault::{bounded_output, entitlements_router_bin, router_output, router_refusal};
use super::account::{
    configured_subscriptions, parse_live_subscriptions, parse_live_subscriptions_any_agent,
    parse_unroutable_accounts, SubscriptionEntry, UnroutableAccount,
};
use super::donation::donated_subscriptions;

/// Enumerate one agent's subscription metadata through the configured
/// acquisition boundary and preserve whether that boundary answered.
///
/// Onboarding needs to distinguish an empty account from an unavailable
/// Skarbiec/entitlements route; flattening both to an empty vector would present
/// a dependency failure as a valid zero-state import.
///
/// This always performs live discovery. The trusted startup catalog belongs to
/// internal routing only and cannot turn a failed user-facing read into success.
pub async fn discover_subscriptions(agent_id: &str) -> Result<Vec<SubscriptionEntry>, String> {
    let broker = entitlements_router_bin();
    let mut entries = live_subscriptions(&broker, agent_id, true).await?;
    for donated in donated_subscriptions(agent_id)? {
        match entries.iter_mut().find(|entry| entry.id == donated.id) {
            Some(existing) => *existing = donated,
            None => entries.push(donated),
        }
    }
    Ok(entries)
}

/// Enumerate one agent's subscription metadata for internal routing.
///
/// A trusted deployment catalog may keep routing available when live discovery
/// fails, but the live failure is always logged and is never used by
/// [`discover_subscriptions`].
pub async fn list_subscriptions(agent_id: &str) -> Vec<SubscriptionEntry> {
    let mut entries = match list_subscriptions_result(agent_id).await {
        Ok(entries) => entries,
        Err(error) => {
            warn!(
                event = "subscription_routing_discovery_failed",
                agent_id,
                %error,
                "live subscription discovery failed and no trusted catalog was usable"
            );
            Vec::new()
        }
    };
    match donated_subscriptions(agent_id) {
        Ok(donated) => {
            for entry in donated {
                match entries.iter_mut().find(|existing| existing.id == entry.id) {
                    Some(existing) => *existing = entry,
                    None => entries.push(entry),
                }
            }
        }
        Err(error) => warn!(
            event = "donated_subscription_overlay_failed",
            agent_id,
            %error,
            "routing continues without the donated-subscriptions overlay"
        ),
    }
    entries
}

/// Every active subscription this deployment holds, whichever agent owns it.
pub async fn list_all_subscriptions() -> Result<Vec<SubscriptionEntry>, String> {
    let output = router_output("list all subscriptions", |command| {
        command.arg("list");
    })
    .await?;
    if !output.status.success() {
        return Err(router_refusal("list all subscriptions", &output));
    }
    parse_live_subscriptions_any_agent(&output.stdout)
}

/// Every subscription account in the vault that carries no `brama:agent:` tag.
///
/// Discovery finds an account by that tag, so an item that loses it stops
/// existing for every caller and every screen while its credential stays
/// perfectly valid. It is the one failure this deployment had no way to see:
/// the account is not expired, not refused and not retired -- it is simply not
/// looked at, and nothing that lists subscriptions can say so, because the
/// listing is the thing that lost it.
///
/// Read through the same `list` the per-agent discovery already shells, so
/// nothing here becomes a second reader of the vault, and metadata only: an
/// item id, a provider and a subscription id, never a value.
pub async fn list_unroutable_accounts() -> Vec<UnroutableAccount> {
    let output = match router_output("list unroutable subscription accounts", |command| {
        command.arg("list");
    })
    .await
    {
        Ok(output) => output,
        Err(error) => {
            warn!(event = "unroutable_account_listing_failed", %error);
            return Vec::new();
        }
    };
    if !output.status.success() {
        warn!(
            event = "unroutable_account_listing_failed",
            error = %router_refusal("list unroutable subscription accounts", &output)
        );
        return Vec::new();
    }
    match parse_unroutable_accounts(&output.stdout) {
        Ok(accounts) => accounts,
        Err(error) => {
            warn!(event = "unroutable_account_listing_failed", %error);
            Vec::new()
        }
    }
}

/// Incomplete Brama credential metadata that still identifies an exact
/// subscription. These entries are renewal candidates, not routable entries:
/// Weles must prove the declared primary maps to the same subscription id, and
/// the resulting donation adds the missing routing tags before any agent sees
/// it.
pub async fn list_recoverable_subscriptions() -> Vec<SubscriptionEntry> {
    list_unroutable_accounts()
        .await
        .into_iter()
        .filter_map(|account| {
            Some(SubscriptionEntry {
                id: account.id?,
                provider: account.provider?,
                status: "active".to_owned(),
                label: None,
                login_item: account.login_item,
            })
        })
        .collect()
}

type LiveSubscriptionsCache = Mutex<HashMap<String, (Instant, Vec<SubscriptionEntry>)>>;

/// Live discovery results per agent. Entries are stored only after a
/// successful listing so a failed shell never poisons the cache.
static LIVE_SUBSCRIPTIONS_CACHE: LazyLock<LiveSubscriptionsCache> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

const LIVE_SUBSCRIPTIONS_CACHE_TTL: Duration = Duration::from_secs(60);

/// Resolve one agent's subscriptions from the vault, serving a fresh cached
/// listing unless `bypass_cache` is set (used when a lookup failed and the
/// caller wants to re-check the vault instead of trusting a stale entry).
async fn live_subscriptions(
    broker: &str,
    agent_id: &str,
    bypass_cache: bool,
) -> Result<Vec<SubscriptionEntry>, String> {
    if !bypass_cache {
        if let Ok(cache) = LIVE_SUBSCRIPTIONS_CACHE.lock() {
            if let Some((fetched_at, entries)) = cache.get(agent_id) {
                if fetched_at.elapsed() < LIVE_SUBSCRIPTIONS_CACHE_TTL {
                    return Ok(entries.clone());
                }
            }
        }
    }
    let entries = list_subscriptions_live(broker, agent_id).await?;
    if let Ok(mut cache) = LIVE_SUBSCRIPTIONS_CACHE.lock() {
        cache.insert(agent_id.to_owned(), (Instant::now(), entries.clone()));
    }
    Ok(entries)
}

/// Shell the entitlements router's bare `list`, which returns a JSON array of
/// every vault item (`{"id","type","tags","updated_at","deleted","versions"}`).
async fn list_subscriptions_live(
    broker: &str,
    agent_id: &str,
) -> Result<Vec<SubscriptionEntry>, String> {
    let output = bounded_output(broker, "list subscriptions", |command| {
        command.arg("list");
    })
    .await?;
    if !output.status.success() {
        return Err(router_refusal("list subscriptions", &output));
    }
    parse_live_subscriptions(&output.stdout, agent_id)
}

async fn list_subscriptions_result(agent_id: &str) -> Result<Vec<SubscriptionEntry>, String> {
    let broker = entitlements_router_bin();
    match live_subscriptions(&broker, agent_id, false).await {
        Ok(mut live) => {
            match configured_subscriptions(agent_id) {
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
            match configured_subscriptions(agent_id) {
                Some(Ok(configured)) => Ok(configured),
                Some(Err(catalog_error)) => Err(format!(
                    "{live_error}; trusted subscription catalog is invalid: {catalog_error}"
                )),
                None => Err(live_error),
            }
        }
    }
}
