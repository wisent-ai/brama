//! Keep the subscriptions Skarbiec actually lists usable without operator actions.
mod claim;
mod reauthorization;
mod renewal;

use crate::gateway::broker;
use crate::subscription_dispatch::usage::{self, RefreshHint};
use reauthorization::schedule_sign_in;
pub(crate) use reauthorization::sign_in_cooldown;
use renewal::{refresh_one, Swept};
use serde_json::{json, Value};
use std::collections::BTreeSet;
use tracing::info;

/// One pass over every subscription Skarbiec lists: each expired grant is
/// renewed, each one that needs a browser gets its sign-in scheduled. How often
/// passes run is the host's Stado schedule for `brama maintain`.
/// An unreadable inventory is the failure, named with the broker's own error.
pub(crate) async fn sweep() -> Result<Value, String> {
    let entries = broker::list_all_subscriptions().await.map_err(|error| {
        format!("the Skarbiec subscription inventory could not be read: {error}")
    })?;
    let mut visited = BTreeSet::new();
    let mut refreshed = 0usize;
    let mut refused = 0usize;
    let mut checks_scheduled = 0usize;
    for entry in entries {
        if entry.status != "active"
            || crate::journal::is_retired(&entry.id)
            || !broker::supports_oauth_refresh(&entry.provider)
            || !visited.insert(entry.id.clone())
        {
            continue;
        }
        match usage::credential_refresh_hint(&entry.id) {
            RefreshHint::AwaitingSignIn => {
                if schedule_sign_in(entry.id, entry.provider) {
                    checks_scheduled += 1;
                }
                continue;
            }
            RefreshHint::NotDue => continue,
            RefreshHint::Read => {}
        }
        match refresh_one(entry.id, entry.provider).await {
            Swept::Refreshed => refreshed += 1,
            Swept::Refused => refused += 1,
            Swept::AwaitingSignIn {
                subscription_id,
                provider,
            } => {
                if schedule_sign_in(subscription_id, provider) {
                    checks_scheduled += 1;
                }
            }
            Swept::NotDue | Swept::Skipped => {}
        }
    }
    info!(
        event = "credential_refresh_sweep_finished",
        subscriptions = visited.len(),
        refreshed,
        refused,
        sign_in_checks_scheduled = checks_scheduled,
        "Finished subscription renewal; scheduled checks are not completed browser logins"
    );
    Ok(json!({
        "subscriptions": visited.len(),
        "refreshed": refreshed,
        "refused": refused,
        "sign_in_checks_scheduled": checks_scheduled,
    }))
}
