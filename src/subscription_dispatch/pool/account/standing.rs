//! Where one subscription's grant stands: its state in the words an operator
//! acts on, the provider's stated expiry, and the refusal in its way.

use serde_json::{json, Value};

use crate::subscription_dispatch::usage;
use crate::subscription_dispatch::usage::{CredentialState, SubscriptionUsage};

/// Where one grant stands, in the four words an operator acts on.
///
/// `burnt` covers both recorded dead states -- a grant the provider disowned and
/// a subscription somebody retired -- because the pool serves neither and no
/// retry changes either; `last_redeem_error` says which of the two it was.
/// `unknown` is the honest answer for a subscription whose grant nothing has
/// ever looked at, which is not the same statement as a working one, and an
/// operator needs the two told apart.
pub(in crate::subscription_dispatch::pool) fn state(
    recorded: Option<&SubscriptionUsage>,
    now_ms: i64,
) -> &'static str {
    let Some(credential) = recorded.and_then(|usage| usage.credential.as_ref()) else {
        return "unknown";
    };
    match credential.state {
        CredentialState::NeedsReauthorization | CredentialState::Disabled => "burnt",
        // An API key states no expiry and never has one, so an absent instant is
        // a live credential rather than an unknown one.
        CredentialState::Active => match credential.expires_at_ms {
            Some(expires_at_ms) if expires_at_ms <= now_ms => "expired",
            _ => "live",
        },
    }
}

/// The provider's stated expiry as an instant a human reads, or `null` when the
/// credential states none.
pub(in crate::subscription_dispatch::pool) fn expires_at(
    recorded: Option<&SubscriptionUsage>,
) -> Value {
    match recorded
        .and_then(|usage| usage.credential.as_ref())
        .and_then(|credential| credential.expires_at_ms)
        .and_then(chrono::DateTime::from_timestamp_millis)
    {
        Some(at) => json!(at.to_rfc3339()),
        None => Value::Null,
    }
}

/// The refusal standing in this credential's way, in the words of whatever
/// refused it, or `null` when nothing refuses it.
///
/// Three records can hold one and the order is what a repair needs. The
/// credential's own cause outranks everything: it is set only while the grant is
/// refused, and a sign-in is the only thing that clears it. A block still in
/// force is next -- the provider said this account is out of quota, which stops
/// a redemption just as effectively for as long as it lasts. A failed check is
/// last, being a verdict about the account rather than about the grant.
///
/// A lapsed block is deliberately not reported. It no longer holds, and a
/// stale refusal printed beside a `live` grant sends an operator looking for
/// a sign-in that nothing needs.
pub(in crate::subscription_dispatch::pool) fn last_redeem_error(
    recorded: Option<&SubscriptionUsage>,
    now_ms: i64,
) -> Value {
    let Some(usage) = recorded else {
        return Value::Null;
    };
    if let Some(cause) = usage
        .credential
        .as_ref()
        .and_then(|credential| credential.cause.as_deref())
    {
        return json!(cause);
    }
    if let Some(block) = usage
        .block
        .as_ref()
        .filter(|block| block.blocked_until_ms > now_ms)
    {
        return json!(block.reason);
    }
    if let Some(detail) = usage::plan_usage_check(Some(usage))
        .filter(|check| !check.ok)
        .and_then(|check| check.detail.as_deref())
    {
        return json!(detail);
    }
    match usage
        .probe
        .as_ref()
        .filter(|probe| !probe.ok)
        .and_then(|probe| probe.detail.as_deref())
    {
        Some(detail) => json!(detail),
        None => Value::Null,
    }
}
