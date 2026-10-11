//! Putting one retired member back in the rotation on this host.
//!
//! The same act the console route performs, so an operator on the machine
//! that holds the pool and a console over HTTP cannot disagree about what a
//! reinstatement is. It changes membership and no credential: the member is
//! offered to dispatch again and still needs a grant of this gateway's own,
//! which is what the sign-in commands obtain.
//!
//! Why it exists: a retirement was permanent. A member given back was
//! skipped by every later command, whatever the accounts in the vault were
//! worth, and no command could say it is used again.

use serde_json::{json, Value};

use crate::gateway::broker;
use crate::subscription_dispatch::usage;

use super::retired;

/// Why a reinstatement was refused, so a caller maps it by kind rather than
/// by the sentence's words.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReinstateRefused {
    /// The request named no member or gave no reason.
    Invalid,
    /// The pool holds no member by that id.
    NoMember,
    /// The member is in the rotation already.
    NotRetired,
    /// The pool could not be read.
    Unreadable,
}

/// Put the retired member `subscription_id` back in the rotation, recording
/// `reason` beside the journal's reinstatement.
pub async fn reinstate_member(
    subscription_id: &str,
    reason: &str,
) -> Result<Value, (ReinstateRefused, String)> {
    let subscription_id = subscription_id.trim();
    if subscription_id.is_empty() {
        return Err((
            ReinstateRefused::Invalid,
            "name the subscription to reinstate".into(),
        ));
    }
    if reason.trim().is_empty() {
        return Err((
            ReinstateRefused::Invalid,
            "a reason must say why this member is used again".into(),
        ));
    }
    let provider = broker::list_all_subscriptions()
        .await
        .map_err(|detail| (ReinstateRefused::Unreadable, detail))?
        .into_iter()
        .find(|entry| entry.id == subscription_id)
        .map(|entry| entry.provider)
        .ok_or_else(|| {
            (
                ReinstateRefused::NoMember,
                format!("this deployment's pool holds no member `{subscription_id}`"),
            )
        })?;
    // Retirement wrote a journal marker and a ledger state, and the pool
    // skips a member for either; a member whose credential the ledger calls
    // disabled is retired whatever the journal says.
    let retirement = retired(subscription_id, usage::usage_for(subscription_id).as_ref())
        .map_err(|detail| (ReinstateRefused::Unreadable, detail))?;
    if !retirement {
        return Err((
            ReinstateRefused::NotRetired,
            format!("`{subscription_id}` is not retired, so there is nothing to reinstate"),
        ));
    }
    crate::journal::reinstate(subscription_id)
        .map_err(|detail| (ReinstateRefused::Unreadable, detail))?;
    usage::record_credential_reinstated(subscription_id, &provider, reason);
    Ok(json!({
        "subscription_id": subscription_id,
        "provider": provider,
        "reason": reason,
        "detail": "the member is in the rotation again; it holds no grant of this gateway's own \
                   until a sign-in obtains one",
    }))
}
