//! Dropping a member the pool only remembers.
//!
//! The pool is what the vault lists, plus what account discovery enrolled
//! from the harnesses' own account commands and from purchase receipts. The
//! usage ledger remembers every member that ever ran through this gateway,
//! and the pool report lists each of those the vault no longer returns as
//! `undiscovered`, with `subscription exists in usage history but was not
//! returned by Skarbiec`. Such a row is not an account this deployment
//! uses: nothing can confirm a credential for it, no sign-in names it, and
//! the only thing it does is make the pool look larger than it is. `disown`
//! cannot take it: that verb retires a member the vault lists and removes
//! its credential, and the vault lists nothing here.
//!
//! This is the verb for exactly that row. It refuses a member the vault
//! still lists (that one is `disown`'s), a member nothing remembers, and a
//! member a live session still leases; otherwise it writes the reason to
//! the journal as a `forgotten` record, drops any overlay and local
//! credential left under the id, and removes the ledger row, so the next
//! pool report no longer lists it. Discovery may enrol the same account
//! again under a new identity, as after a reconciliation removal: a
//! forgotten id is a tombstone, not a ban on the account.

use serde_json::{json, Value};

use crate::gateway::broker;
use crate::subscription_dispatch::{leases, usage};

/// Why forgetting was refused, so a caller maps it by kind rather than by
/// the sentence's words.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForgetRefused {
    /// The request named no member or gave no reason.
    Invalid,
    /// Neither the vault nor the usage history knows the id.
    NoRecord,
    /// The vault lists the member: it is in the pool, so `disown` is the
    /// verb that takes it out.
    Listed,
    /// A live session still leases the member.
    Leased,
    /// The pool, the lease register or the journal could not be read or
    /// written.
    Unreadable,
}

/// Drop one member that exists only in the usage history, for a reason.
pub async fn forget_member(
    subscription_id: &str,
    reason: &str,
) -> Result<Value, (ForgetRefused, String)> {
    let subscription_id = subscription_id.trim();
    if subscription_id.is_empty() {
        return Err((
            ForgetRefused::Invalid,
            "name the subscription to forget".into(),
        ));
    }
    let reason = reason.trim();
    if reason.is_empty() {
        return Err((
            ForgetRefused::Invalid,
            "a reason must say why this member is forgotten".into(),
        ));
    }
    let listed = broker::list_all_subscriptions()
        .await
        .map_err(|detail| (ForgetRefused::Unreadable, detail))?
        .into_iter()
        .find(|entry| entry.id == subscription_id);
    if let Some(entry) = listed {
        return Err((
            ForgetRefused::Listed,
            format!(
                "the vault lists `{subscription_id}` ({}) as a member of the pool, so it is not a \
                 memory: give it back with `brama subscription disown --subscription-id \
                 {subscription_id} --reason …`",
                entry.provider
            ),
        ));
    }
    let Some(recorded) = usage::usage_for(subscription_id) else {
        return Err((
            ForgetRefused::NoRecord,
            format!(
                "neither the vault nor the usage history knows `{subscription_id}`; there is \
                 nothing to forget"
            ),
        ));
    };
    let provider = recorded.provider.trim().to_owned();
    let leased: Vec<String> = leases::live()
        .map_err(|detail| (ForgetRefused::Unreadable, detail))?
        .into_iter()
        .filter(|lease| lease.subscription_id == subscription_id)
        .map(|lease| lease.session_id)
        .collect();
    if !leased.is_empty() {
        return Err((
            ForgetRefused::Leased,
            format!(
                "`{subscription_id}` still carries {} live session lease(s) ({}); release them \
                 first with `brama subscription lease release`",
                leased.len(),
                leased.join(", ")
            ),
        ));
    }
    crate::journal::forget(subscription_id, reason)
        .map_err(|detail| (ForgetRefused::Unreadable, detail))?;
    broker::donated_remove(subscription_id)
        .map_err(|detail| (ForgetRefused::Unreadable, detail))?;
    broker::remove_donated_credential(&provider, subscription_id)
        .map_err(|detail| (ForgetRefused::Unreadable, detail))?;
    usage::remove_subscription(subscription_id)
        .map_err(|detail| (ForgetRefused::Unreadable, detail))?;
    Ok(json!({
        "subscription_id": subscription_id,
        "provider": provider,
        "reason": reason,
        "detail": "the member existed only in the usage history and is forgotten: it leaves the \
                   pool report, the reason stands in the journal, and discovery may enrol the \
                   account again under its own identity",
    }))
}
