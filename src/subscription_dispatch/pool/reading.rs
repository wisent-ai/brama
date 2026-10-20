//! Why one row of the pool document is not a healthy answer, in the words of
//! whatever produced the refusal.

use serde_json::Value;
use wisent_errors::{Code, Failure};

use crate::core::failure;
use crate::gateway::broker::SubscriptionEntry;
use crate::subscription_dispatch::usage;
use crate::subscription_dispatch::usage::{CredentialState, SubscriptionUsage};

use super::account::retired;

pub(super) fn failure_json(failure: &Failure) -> Value {
    serde_json::from_str(&failure.to_json()).expect("Wisent failure serialization is JSON")
}

/// Why one row of the document is not a healthy answer, in the words of
/// whatever produced the refusal. Every row is a member the vault lists, or
/// a retired one the usage history remembers.
pub(super) fn reading_error(
    entry: &SubscriptionEntry,
    recorded: Option<&SubscriptionUsage>,
    windows: &usage::PlanWindows,
    now: i64,
) -> Option<Value> {
    if entry.status != "active" || retired(&entry.id, recorded) != Ok(false) {
        return None;
    }
    let check = usage::plan_usage_check(recorded);
    if check.is_some_and(|check| !check.ok) {
        if let Some(failure) = recorded.and_then(|recorded| recorded.usage_failure.as_ref()) {
            return Some(failure.clone());
        }
    }
    let credential = recorded.and_then(|recorded| recorded.credential.as_ref());
    // The code follows from which recorded state answers, never from the
    // sentence: a grant the provider disowned is authentication; every other
    // state below recorded no class, so it is unattributed.
    let (code, detail) = if let Some(cause) = credential
        .filter(|credential| credential.state == CredentialState::NeedsReauthorization)
        .and_then(|credential| credential.cause.as_deref())
    {
        (wisent_errors::Code::Auth, cause)
    } else if let Some(check) = check.filter(|check| !check.ok) {
        // A failed check the ledger recorded no reason for says that it
        // failed and nothing more.
        let reason = match check.detail.as_deref() {
            Some(reason) => reason,
            None => "usage refresh failed without a stated reason",
        };
        (wisent_errors::Code::Unknown, reason)
    } else if windows.stale {
        (
            wisent_errors::Code::Unknown,
            "the last usage reading is no longer current; refresh usage to obtain a new report",
        )
    } else if windows.limits.is_empty() {
        if check.is_some_and(|check| check.ok)
            && !crate::providers::adapter::publishes_plan_usage(&entry.provider)
        {
            return None;
        }
        if check.is_some() {
            (
                wisent_errors::Code::Unknown,
                "no current plan windows are available from the last usage reading",
            )
        } else {
            (wisent_errors::Code::Unknown, "usage has not been read for this subscription")
        }
    } else {
        return None;
    };
    let refused = failure::envelope(
        "brama.subscriptions.usage",
        code,
        "one subscription usage report",
        detail,
    )
    .with_context("subscription", &entry.id)
    .with_context("provider", &entry.provider);
    // The time of the read this row describes, or, for a subscription never
    // read, the time this document observed that it was not.
    let refused = match check {
        Some(check) => refused.with_context("attempted_at_ms", check.attempted_at_ms.to_string()),
        None => refused.with_context("observed_at_ms", now.to_string()),
    };
    Some(failure_json(&refused))
}
