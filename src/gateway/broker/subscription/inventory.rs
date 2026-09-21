//! Which subscriptions exist, for one agent and for this deployment.
//!
//! Discovery is one question with several audiences, and the difference
//! between them is the whole subject: a user-facing import must not present an
//! unavailable Skarbiec as an empty account, internal routing may keep going
//! on the trusted catalog, the console wants every agent's rows, and renewal
//! wants the accounts the listing itself has lost. The per-agent cache lives
//! here too, because a listing is the only thing worth caching -- it is read
//! per request and a failed shell must never be what a later caller sees.

use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use tracing::warn;

use super::super::vault::{entitlements_router_bin, existing_item_account, raw_listing};
use super::account::{
    configured_subscription_ids, configured_subscriptions, parse_live_subscriptions,
    parse_owned_subscriptions, parse_unroutable_accounts, SubscriptionEntry, UnroutableAccount,
};
use super::donation::donated_subscriptions;

/// Enumerate the subscription pool through the configured acquisition
/// boundary and preserve whether that boundary answered.
///
/// Onboarding needs to distinguish an empty pool from an unavailable
/// Skarbiec/entitlements route; flattening both to an empty vector would present
/// a dependency failure as a valid zero-state import.
///
/// This always performs live discovery. The trusted startup catalog belongs to
/// internal routing only and cannot turn a failed user-facing read into success.
/// `agent_id` names the caller for the log; the pool is the same for every one.
pub async fn discover_subscriptions(agent_id: &str) -> Result<Vec<SubscriptionEntry>, String> {
    let broker = entitlements_router_bin();
    let mut entries = live_subscriptions(&broker, true).await?;
    for donated in donated_subscriptions(None)? {
        match entries.iter_mut().find(|entry| entry.id == donated.id) {
            Some(existing) => *existing = donated,
            None => entries.push(donated),
        }
    }
    let _ = agent_id;
    Ok(entries)
}

/// The subscriptions one agent banked itself, from the vault's provenance tag
/// and the overlay rows it wrote. This answers who may retire an account; it
/// never narrows routing.
pub async fn owned_subscriptions(agent_id: &str) -> Result<Vec<SubscriptionEntry>, String> {
    let stdout = raw_listing(&entitlements_router_bin(), "list owned subscriptions").await?;
    let mut entries = parse_owned_subscriptions(&stdout, agent_id)?;
    for donated in donated_subscriptions(Some(agent_id))? {
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
    match donated_subscriptions(None) {
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

/// Every active subscription this deployment holds. Since 2026-09-16 this is
/// the same pool every caller routes over; it stays a separate reader because
/// it never consults the trusted boot catalog or a donated overlay.
pub async fn list_all_subscriptions() -> Result<Vec<SubscriptionEntry>, String> {
    let stdout = raw_listing(&entitlements_router_bin(), "list all subscriptions").await?;
    Ok(with_recorded_accounts(parse_live_subscriptions(&stdout)?).await)
}

/// Subscriptions the vault holds, completely tagged, that this process was
/// not started with.
///
/// The runtime policy is generated from the vault's tags when a release is
/// installed on a host, and the launcher builds the boot catalogue out of the
/// subscriptions that policy names. An account added to the vault after that
/// install is in neither, so the gateway does not merely fail to redeem it -
/// it has never heard of it. On 2026-09-20 three paid Claude accounts and two
/// Codex accounts sat in `charless-mac-mini`'s vault, correctly tagged and
/// routed, while every request answered `no working subscription model for
/// signed agent` and nothing in the readiness answer said why.
///
/// Read from the same live listing the pool uses, against the boot catalogue
/// the launcher exported. An empty catalogue means this process was started
/// without one (a standalone or a test), where every account would look
/// unnamed; that says nothing, so nothing is reported.
pub async fn policy_unnamed_subscriptions() -> Vec<SubscriptionEntry> {
    let configured = configured_subscription_ids();
    if configured.is_empty() {
        return Vec::new();
    }
    match list_all_subscriptions().await {
        Ok(entries) => entries
            .into_iter()
            .filter(|entry| !configured.contains(&entry.id))
            .collect(),
        Err(error) => {
            warn!(event = "policy_unnamed_subscription_census_failed", %error);
            Vec::new()
        }
    }
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
    let stdout = match raw_listing(
        &entitlements_router_bin(),
        "list unroutable subscription accounts",
    )
    .await
    {
        Ok(stdout) => stdout,
        Err(error) => {
            warn!(event = "unroutable_account_listing_failed", %error);
            return Vec::new();
        }
    };
    match parse_unroutable_accounts(&stdout) {
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
                account: account.account,
            })
        })
        .collect()
}

type LiveSubscriptionsCache = Mutex<Option<(Instant, Vec<SubscriptionEntry>)>>;

/// The live pool listing. Stored only after a successful listing so a failed
/// shell never poisons the cache.
static LIVE_SUBSCRIPTIONS_CACHE: LazyLock<LiveSubscriptionsCache> =
    LazyLock::new(|| Mutex::new(None));

const LIVE_SUBSCRIPTIONS_CACHE_TTL: Duration = Duration::from_secs(60);

/// Resolve the pool from the vault, serving a fresh cached listing unless
/// `bypass_cache` is set (used when a lookup failed and the caller wants to
/// re-check the vault instead of trusting a stale entry).
async fn live_subscriptions(
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
async fn with_recorded_accounts(mut entries: Vec<SubscriptionEntry>) -> Vec<SubscriptionEntry> {
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
async fn list_subscriptions_live(broker: &str) -> Result<Vec<SubscriptionEntry>, String> {
    let stdout = raw_listing(broker, "list subscriptions").await?;
    Ok(with_recorded_accounts(parse_live_subscriptions(&stdout)?).await)
}

async fn list_subscriptions_result(agent_id: &str) -> Result<Vec<SubscriptionEntry>, String> {
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
