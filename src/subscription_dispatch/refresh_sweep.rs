//! Keep the subscriptions Skarbiec actually lists usable without operator actions.
mod cadence;
mod claim;
mod reauthorization;
mod renewal;

use crate::gateway::broker;
use crate::subscription_dispatch::usage::{self, RefreshHint};
pub use cadence::spawn;
use reauthorization::schedule_sign_in;
pub(crate) use reauthorization::sign_in_cooldown;
use renewal::{refresh_one, Swept};
use std::collections::BTreeSet;
use std::time::Duration;
use tracing::{info, warn};

async fn sweep(skew: Duration) {
    let entries = match broker::list_all_subscriptions().await {
        Ok(entries) => entries,
        Err(error) => {
            warn!(event = "subscription_inventory_failed", operation = "skarbiec.list", %error,
                "Automatic renewal could not read the current Skarbiec subscription inventory");
            return;
        }
    };
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
        match usage::credential_refresh_hint(&entry.id, skew) {
            RefreshHint::AwaitingSignIn => {
                if schedule_sign_in(entry.id, entry.provider) {
                    checks_scheduled += 1;
                }
                continue;
            }
            RefreshHint::NotDue => continue,
            RefreshHint::Read => {}
        }
        match refresh_one(entry.id, entry.provider, skew).await {
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
}
