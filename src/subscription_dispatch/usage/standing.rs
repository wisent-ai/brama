//! What the ledger alone says stops a request from presenting one
//! subscription, read before anything is spent on it.
//!
//! The request path pays for a member three times before it learns the ledger
//! already refused it: a vault child to list the pool, a broker redemption to
//! read the credential, and a provider round trip to discover its models or to
//! be refused again. The rotation walk reads the block and the credential
//! state; model discovery read only the state, so a member inside a
//! re-authorization block whose refreshed grant the ledger calls active was
//! read and discovered on every request and then skipped by the walk. One
//! verdict, read by both, is what makes "this member is not presented" true
//! on both.

use crate::types::{GatewayRefusal, ProviderRefusal, Refusal};

use super::{awaiting_sign_in_cause, now_ms, read_ledger};

/// The refusal the ledger already holds against `subscription_id`, with the
/// class the request is answered with when nothing else can serve it, or
/// `None` when the ledger has nothing standing against it.
///
/// The credential state comes first: a grant the provider disowned or an
/// operator retired is refused however its block stands. A standing block is
/// then read for what it is — a spent paid balance, which no wait repairs, or
/// a rate window, which one does — and names the hour it lifts, because a
/// refusal that omits it is a wait nobody can plan around. The sentences are
/// the ledger's own: the provider's refusal as it was recorded, never a
/// paraphrase.
pub fn standing_refusal(subscription_id: &str) -> Option<Refusal> {
    if let Some(cause) = awaiting_sign_in_cause(subscription_id) {
        return Some(Refusal::gateway(
            GatewayRefusal::SubscriptionReauthorizationRequired,
            format!("awaiting sign-in: {cause}"),
        ));
    }
    let now = now_ms();
    read_ledger(|ledger| {
        let block = ledger
            .subscriptions
            .get(subscription_id)?
            .block
            .as_ref()
            .filter(|block| block.blocked_until_ms > now)?;
        let lifts = block_instant(block.blocked_until_ms);
        if block.quota_exhausted {
            return Some(Refusal::new(
                ProviderRefusal::QuotaExhausted,
                format!(
                    "paid balance spent, recorded until {lifts}: {}",
                    block.reason
                ),
            ));
        }
        Some(Refusal::new(
            ProviderRefusal::RateLimited,
            format!("rate limited, the block lifts at {lifts}: {}", block.reason),
        ))
    })
}

/// One ledger instant as a reader can act on it: UTC, to the second; an
/// instant chrono cannot place is named in the ledger's own milliseconds.
fn block_instant(milliseconds: i64) -> String {
    match chrono::DateTime::from_timestamp_millis(milliseconds) {
        Some(at) => at.format("%Y-%m-%dT%H:%M:%SZ").to_string(),
        None => format!("{milliseconds}ms since the epoch, outside the calendar"),
    }
}
