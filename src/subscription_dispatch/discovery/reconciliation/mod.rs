//! Reconcile obsolete membership from recorded identities, never member labels.
use crate::gateway::broker::{self, SubscriptionEntry};
use crate::subscription_dispatch::{leases, usage};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

pub(super) async fn reconcile(members: &mut Vec<SubscriptionEntry>) -> Result<Vec<Value>, String> {
    let recorded = usage::recorded_subscriptions();
    let leased = leases::live()?;
    let mut retained = BTreeMap::new();
    let mut obsolete = BTreeMap::new();
    // Each member's retirement is read once; an unreadable journal stops the
    // reconciliation before anything is removed.
    let mut retired = BTreeMap::new();
    for member in members.iter() {
        retired.insert(member.id.clone(), crate::journal::is_retired(&member.id)?);
    }
    members.sort_by(|left, right| {
        retired
            .get(&left.id)
            .cmp(&retired.get(&right.id))
            .then_with(|| left.id.cmp(&right.id))
    });
    for member in members.iter() {
        let account = member
            .account
            .as_deref()
            .filter(|account| !account.trim().is_empty());
        if crate::journal::membership_was_removed(&member.id)? {
            obsolete.insert(
                member.id.clone(),
                (
                    member.provider.clone(),
                    account.map(str::to_owned),
                    "unfinished membership removal",
                ),
            );
            continue;
        }
        if recorded
            .get(&member.id)
            .and_then(|row| row.credential.as_ref())
            .is_some_and(|credential| credential.borrowed_from.is_some())
        {
            obsolete.insert(
                member.id.clone(),
                (
                    member.provider.clone(),
                    account.map(str::to_owned),
                    "borrowed grant membership",
                ),
            );
            continue;
        }
        if let Some(account) = account {
            let key = (member.provider.clone(), account.to_lowercase());
            if let Some(keeper) = retained.get(&key) {
                if keeper != &member.id {
                    obsolete.insert(
                        member.id.clone(),
                        (
                            member.provider.clone(),
                            Some(account.to_owned()),
                            "duplicate account",
                        ),
                    );
                }
            } else {
                retained.insert(key, member.id.clone());
            }
        } else {
            obsolete.insert(
                member.id.clone(),
                (member.provider.clone(), None, "missing account identity"),
            );
        }
    }
    let live_ids: BTreeSet<_> = members.iter().map(|member| member.id.as_str()).collect();
    for (id, entry) in &recorded {
        if !live_ids.contains(id.as_str()) && entry.discovery.is_none() {
            obsolete.insert(
                id.clone(),
                (
                    entry.provider.clone(),
                    None,
                    "ledger-only membership without an account",
                ),
            );
        }
    }
    let mut removed = Vec::new();
    for (id, (provider, account, reason)) in obsolete {
        if leased.iter().any(|lease| lease.subscription_id == id) {
            return Err(format!("cannot reconcile subscription {id}: active session leases remain; completed removals: {removed:?}"));
        }
        crate::journal::remove_membership(&id)?;
        if !crate::journal::is_retired(&id)? {
            return Err(format!(
                "cannot reconcile subscription {id}: retirement was not persisted"
            ));
        }
        if leases::live()?
            .iter()
            .any(|lease| lease.subscription_id == id)
        {
            return Err(format!("subscription {id} retired but not removed: a session acquired its lease during reconciliation"));
        }
        broker::remove_subscription_item(&provider, &id, account.as_deref())
            .await
            .map_err(|error| {
                format!("reconcile subscription {id}: {error}; completed removals: {removed:?}")
            })?;
        broker::remove_donated_credential(&provider, &id)?;
        broker::donated_remove(&id)?;
        usage::remove_subscription(&id)?;
        members.retain(|member| member.id != id);
        removed.push(json!({"id": id, "provider": provider, "account": account, "reason": reason}));
    }
    Ok(removed)
}
