//! What the pool says about one subscription.
//!
//! One row shape serves the HTTP listing, the pool document, the CLI and the
//! refresh, because a second projection of an account is a second thing to be
//! wrong about it. The words are chosen for someone deciding what to repair:
//! where the grant stands, what refused it, and until when it is good.

use serde_json::{json, Value};

use crate::gateway::broker::SubscriptionEntry;
use crate::subscription_dispatch::usage;
use crate::subscription_dispatch::usage::{CredentialState, SubscriptionUsage};

/// The provider named for a subscription whose record carries none, so a row is
/// never keyed on an empty string.
const UNATTRIBUTED: &str = "unattributed";

/// One account projection shared by the HTTP list, pool, CLI and refresh.
pub fn subscription_view(entry: &SubscriptionEntry) -> Value {
    let recorded = usage::usage_for(&entry.id);
    let windows = usage::plan_windows(recorded.as_ref());
    subscription_row(entry, recorded.as_ref(), windows)
}

pub(super) fn subscription_row(
    entry: &SubscriptionEntry,
    recorded: Option<&SubscriptionUsage>,
    windows: usage::PlanWindows,
) -> Value {
    json!({
        "id": entry.id,
        "provider": entry.provider,
        "status": entry.status,
        "label": entry.label,
        "login_item": entry.login_item,
        "sign_in": crate::journal::latest_subscription_sign_in(&entry.id),
        "automatic_sign_in": automatic_sign_in_view(entry),
        "limits": windows.limits,
        "measured": recorded.map(|usage| &usage.measured),
        "block": recorded.and_then(|usage| usage.block.as_ref()),
        "observed_at_ms": recorded.and_then(|usage| usage.updated_at_ms),
        "probe": recorded.and_then(|usage| usage.probe.as_ref()),
        "usage_check": usage::plan_usage_check(recorded),
        "credential": credential_view(entry, recorded),
        "usage_source": windows.source.map(|source| source.as_str()),
        "stale": windows.stale,
    })
}

/// Display observations from the authentication run. A missing optional tag
/// says nothing about whether Skarbiec can resolve the account.
fn automatic_sign_in_view(entry: &SubscriptionEntry) -> Value {
    let applies = crate::subscription_dispatch::sign_in::weles_provider(&entry.provider).is_some()
        && entry.status == "active"
        && !crate::journal::is_retired(&entry.id);
    let failure = applies
        .then(|| crate::subscription_dispatch::sign_in::observed_failure(&entry.id))
        .flatten();
    let latest = crate::journal::latest_subscription_sign_in(&entry.id);
    json!({
        "applies": applies,
        "automatic": applies && failure.is_none(),
        "state": if !applies { "not_applicable" } else if failure.is_some() { "failed" }
            else if latest.is_some() { "succeeded" } else { "not_observed" },
        "blocked_by": failure.as_ref().map(|failure| failure.code()),
        "detail": failure.as_ref().map(|failure| failure.detail()),
        "last_attempt": latest,
    })
}

fn credential_view(entry: &SubscriptionEntry, recorded: Option<&SubscriptionUsage>) -> Value {
    let Some(credential) = recorded.and_then(|usage| usage.credential.as_ref()) else {
        return Value::Null;
    };
    let state = if entry.status == "retired" || crate::journal::is_retired(&entry.id) {
        CredentialState::Disabled
    } else {
        credential.state
    };
    json!({
        "state": state.as_str(),
        "cause": credential.cause,
        "recorded_at_ms": credential.recorded_at_ms,
        "expires_at_ms": credential.expires_at_ms,
        "refreshed_at_ms": credential.refreshed_at_ms,
    })
}

/// Where one grant stands, in the four words an operator acts on.
///
/// `burnt` covers both recorded dead states -- a grant the provider disowned and
/// a subscription somebody retired -- because the pool serves neither and no
/// retry changes either; `last_redeem_error` says which of the two it was.
/// `unknown` is the honest answer for a subscription whose grant nothing has
/// ever looked at, which is not the same statement as a working one; that
/// distinction is exactly what the burnt-codex morning needed and did not have.
pub(super) fn state(recorded: Option<&SubscriptionUsage>, now_ms: i64) -> &'static str {
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
pub(super) fn expires_at(recorded: Option<&SubscriptionUsage>) -> Value {
    recorded
        .and_then(|usage| usage.credential.as_ref())
        .and_then(|credential| credential.expires_at_ms)
        .and_then(chrono::DateTime::from_timestamp_millis)
        .map(|at| json!(at.to_rfc3339()))
        .unwrap_or(Value::Null)
}

/// The refusal standing in this credential's way, in the words of whatever
/// refused it.
///
/// Three records can hold one and the order is what a repair needs. The
/// credential's own cause outranks everything: it is set only while the grant is
/// refused, and a sign-in is the only thing that clears it. A block still in
/// force is next -- the provider said this account is out of quota, which stops
/// a redemption just as effectively for as long as it lasts. A failed check is
/// last, being a verdict about the account rather than about the grant.
///
/// A lapsed block is deliberately not reported. It was true an hour ago and is
/// not now, and a stale refusal printed beside a `live` grant is what sends an
/// operator looking for a sign-in that nothing needs.
pub(super) fn last_redeem_error(recorded: Option<&SubscriptionUsage>, now_ms: i64) -> Value {
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
    usage
        .probe
        .as_ref()
        .filter(|probe| !probe.ok)
        .and_then(|probe| probe.detail.as_deref())
        .map_or(Value::Null, |detail| json!(detail))
}

/// Whether this subscription was deliberately taken out of the pool.
pub(super) fn retired(subscription_id: &str, recorded: Option<&SubscriptionUsage>) -> bool {
    crate::journal::is_retired(subscription_id)
        || recorded
            .and_then(|usage| usage.credential.as_ref())
            .is_some_and(|credential| credential.state == CredentialState::Disabled)
}

pub(super) fn named(provider: &str) -> String {
    let trimmed = provider.trim();
    if trimmed.is_empty() {
        return UNATTRIBUTED.to_string();
    }
    trimmed.to_string()
}

pub(super) fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}
