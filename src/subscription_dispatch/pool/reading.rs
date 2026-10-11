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
/// whatever produced the refusal.
///
/// A subscription the ledger remembers but the vault no longer lists is its own
/// case: nothing can confirm its credential, which is a different problem from a
/// grant that was refused.
pub(super) fn reading_error(
    entry: &SubscriptionEntry,
    recorded: Option<&SubscriptionUsage>,
    windows: &usage::PlanWindows,
    now: i64,
) -> Option<Value> {
    if entry.status == "undiscovered" {
        return Some(failure_json(&failure::envelope(
            "brama.subscriptions.discovery",
            Code::Config,
            "one subscription usage report",
            "subscription exists in usage history but was not returned by Skarbiec; its current credential cannot be confirmed",
        )
        .with_context("subscription", &entry.id)
        .with_context("provider", &entry.provider)
        .with_context("attempted_at_ms", now.to_string())));
    }
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
    let detail = if let Some(cause) = credential
        .filter(|credential| credential.state == CredentialState::NeedsReauthorization)
        .and_then(|credential| credential.cause.as_deref())
    {
        cause
    } else if let Some(check) = check.filter(|check| !check.ok) {
        check
            .detail
            .as_deref()
            .unwrap_or("usage refresh failed without a stated reason")
    } else if windows.stale {
        "the last usage reading is no longer current; refresh usage to obtain a new report"
    } else if windows.limits.is_empty() {
        if check.is_some_and(|check| check.ok)
            && !crate::providers::adapter::publishes_plan_usage(&entry.provider)
        {
            return None;
        }
        if check.is_some() {
            "no current plan windows are available from the last usage reading"
        } else {
            "usage has not been read for this subscription"
        }
    } else {
        return None;
    };
    Some(failure_json(
        &failure::envelope(
            "brama.subscriptions.usage",
            failure::code_for_message(detail, "provider_failure"),
            "one subscription usage report",
            detail,
        )
        .with_context("subscription", &entry.id)
        .with_context("provider", &entry.provider)
        .with_context(
            "attempted_at_ms",
            check.map_or(now, |check| check.attempted_at_ms).to_string(),
        ),
    ))
}
