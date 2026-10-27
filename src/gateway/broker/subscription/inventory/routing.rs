//! Request admission reads the last observed pool, never waits for a vault child.
//! One background refresh owns discovery; pending and failed discovery are refusals.

use std::sync::{LazyLock, Mutex};

use super::super::account::SubscriptionEntry;
use super::super::donation::donated_subscriptions;
use super::listing::list_subscriptions_result;

type InventoryResult = Result<Vec<SubscriptionEntry>, String>;

#[derive(Default)]
struct Snapshot {
    result: Option<InventoryResult>,
    refreshing: bool,
}

static SNAPSHOT: LazyLock<Mutex<Snapshot>> = LazyLock::new(|| Mutex::new(Snapshot::default()));

/// The observed routing pool. The agent names the refresh log, not a separate pool.
/// Credential redemption remains authoritative at dispatch; metadata grants no access.
pub fn routing_subscriptions(agent_id: &str) -> InventoryResult {
    // The snapshot value is replaced whole, so a poisoned lock is taken back
    // rather than refusing every request and never refreshing again.
    let mut snapshot = crate::core::sync::held(&SNAPSHOT, "routing inventory snapshot");
    let result = match &snapshot.result {
        Some(result) => result.clone(),
        None => Err(
            "subscription inventory discovery has not completed; operation: list subscriptions"
                .into(),
        ),
    };
    if !snapshot.refreshing {
        snapshot.refreshing = true;
        let agent_id = agent_id.to_owned();
        tokio::spawn(async move {
            // A failed discovery is logged with its cause by the refresh and
            // published to the snapshot; the detached task records that it
            // finished, and how, rather than dropping the answer.
            let published = refresh_routing_subscriptions(&agent_id).await;
            tracing::debug!(
                event = "routing_inventory_refreshed",
                discovered = published.is_ok(),
                "detached routing inventory refresh finished"
            );
        });
    }
    result
}

/// Explicit discovery for standalone diagnostics, which have no preceding service sweep.
pub async fn refresh_routing_subscriptions(agent_id: &str) -> InventoryResult {
    let result = read_inventory(agent_id).await;
    if let Err(error) = &result {
        tracing::warn!(event = "routing_inventory_discovery_failed", %error, "routing refuses without waiting for discovery");
    }
    let mut snapshot = crate::core::sync::held(&SNAPSHOT, "routing inventory snapshot");
    snapshot.result = Some(result.clone());
    snapshot.refreshing = false;
    result
}

async fn read_inventory(agent_id: &str) -> InventoryResult {
    let mut entries = list_subscriptions_result(agent_id).await?;
    for entry in donated_subscriptions(None)? {
        match entries.iter_mut().find(|existing| existing.id == entry.id) {
            Some(existing) => *existing = entry,
            None => entries.push(entry),
        }
    }
    Ok(entries)
}
