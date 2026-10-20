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
/// renewed, each one that needs a browser gets its sign-in scheduled through
/// Weles. Each maintenance pass of the serving process runs it once.
/// An unreadable inventory is the failure, named with the broker's own error.
pub(crate) async fn sweep() -> Result<Value, String> {
    let entries = broker::list_all_subscriptions().await.map_err(|error| {
        format!("the Skarbiec subscription inventory could not be read: {error}")
    })?;
    let mut visited = BTreeSet::new();
    let mut refreshed = Vec::new();
    let mut refused = Vec::new();
    let mut checks_scheduled = Vec::new();
    for entry in entries {
        if entry.status != "active"
            || crate::journal::is_retired(&entry.id)?
            || !broker::supports_oauth_refresh(&entry.provider)
            || !visited.insert(entry.id.clone())
        {
            continue;
        }
        match usage::credential_refresh_hint(&entry.id) {
            RefreshHint::AwaitingSignIn => {
                if schedule_sign_in(entry.id.clone(), entry.provider) {
                    checks_scheduled.push(entry.id);
                }
                continue;
            }
            RefreshHint::NotDue => continue,
            RefreshHint::Read => {}
        }
        match refresh_one(entry.id.clone(), entry.provider).await {
            Swept::Refreshed => refreshed.push(entry.id),
            Swept::Refused => refused.push(entry.id),
            Swept::AwaitingSignIn {
                subscription_id,
                provider,
            } => {
                if schedule_sign_in(subscription_id.clone(), provider) {
                    checks_scheduled.push(subscription_id);
                }
            }
            Swept::NotDue | Swept::Skipped => {}
        }
    }
    // A vault item with an exact account identity but no routing mark is
    // outside the pool, so nothing above saw it. Its mark is restored and
    // its sign-in scheduled, so the account gets its own grant without
    // anybody retagging the item by hand; one that cannot be admitted is
    // reported with its cause and left for the next pass.
    let mut admitted = Vec::new();
    let mut refused_admission = Vec::new();
    for account in broker::list_unroutable_accounts().await? {
        let Some(id) = account.id.clone() else {
            continue;
        };
        if visited.contains(&id) || crate::journal::is_retired(&id)? {
            continue;
        }
        match crate::subscription_dispatch::sign_in::admit_recoverable(&id).await {
            Ok(Some(entry)) => {
                visited.insert(entry.id.clone());
                admitted.push(
                    json!({ "id": entry.id, "provider": entry.provider, "item": account.item }),
                );
                if broker::supports_oauth_refresh(&entry.provider)
                    && schedule_sign_in(entry.id.clone(), entry.provider)
                {
                    checks_scheduled.push(entry.id);
                }
            }
            Ok(None) => {}
            Err(detail) => {
                refused_admission.push(json!({ "id": id, "item": account.item, "detail": detail }))
            }
        }
    }
    info!(
        event = "credential_refresh_sweep_finished",
        subscriptions = visited.len(),
        refreshed = refreshed.len(),
        refused = refused.len(),
        sign_in_checks_scheduled = checks_scheduled.len(),
        recovered = admitted.len(),
        "Finished subscription renewal; scheduled checks are not completed browser logins"
    );
    Ok(json!({
        "subscriptions": visited.len(),
        "refreshed": refreshed.len(),
        "refused": refused.len(),
        "sign_in_checks_scheduled": checks_scheduled.len(),
        "recovered": admitted,
        "unrecoverable": refused_admission,
    }))
}
