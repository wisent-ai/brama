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
    let mut snapshot = SNAPSHOT
        .lock()
        .map_err(|error| format!("routing inventory state is unavailable: {error}"))?;
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
            let _ = refresh_routing_subscriptions(&agent_id).await;
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
    let mut snapshot = SNAPSHOT
        .lock()
        .map_err(|error| format!("routing inventory state could not publish discovery: {error}"))?;
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
