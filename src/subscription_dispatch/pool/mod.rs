//! What the subscription pool holds, stated from what is already recorded.
//!
//! Browser automation across the company stopped for most of a working day
//! because this pool was empty. Both codex subscription credentials were burnt,
//! every `best`-aliased call answered `429 subscription_unavailable`, and the
//! only way to learn which of the two facts was true -- an exhausted plan or a
//! disowned grant -- was to grep `brama-always-on.err` for the code and read the
//! timestamps by hand. The gateway had known since its first refresh sweep: the
//! ledger already carried `needs_reauthorization` against both grants with the
//! provider's own sentence beside them, and no command in the product would say
//! it out loud. This document says it.
//!
//! [`report`] contacts no provider and redeems no capability -- it joins the
//! deployment's subscription listing to the ledger and states what is already
//! recorded -- so it is safe to run against a gateway that is serving traffic.
//! Acting on what it says is `refresh`, which rotates grants and therefore
//! demands a reason and leaves an audit record. Neither can print credential
//! material: this reads the ledger, which has never held any.
//!
//! `scope` holds how much of the pool one proven identity may be told about,
//! and `account` holds the one row shape every audience reads.

mod account;
mod refresh;
mod scope;

use std::collections::BTreeMap;

use serde_json::{json, Value};
use wisent_errors::{Code, Failure};

use crate::core::failure;
use crate::gateway::broker::{self, SubscriptionEntry};
use crate::subscription_dispatch::usage;
use crate::subscription_dispatch::usage::{CredentialState, SubscriptionUsage};

use account::{expires_at, last_redeem_error, named, now_ms, retired, state, subscription_row};

pub use account::subscription_view;
pub use refresh::{refresh_provider, refresh_subscription};
pub use scope::PoolScope;

/// The pool as the gateway sees it, narrowed to what the caller proved.
///
/// This contacts no provider and redeems no capability -- it joins the
/// deployment's subscription listing to the ledger and states what is already
/// recorded -- so it is safe against a gateway that is serving traffic.
/// Bringing each plan reading current from the provider's own usage report is
/// the plan-usage capability beside it,
/// [`plan_usage::report`](crate::subscription_dispatch::plan_usage::report),
/// which reads those reports and then answers this same document.
pub async fn report(scope: &PoolScope) -> Value {
    let (entries, errors) = inventory(scope).await;
    document(scope, &entries, errors)
}

/// The one pool document, whoever asked and whatever was read to answer it.
///
/// Both capabilities end here deliberately. They differ in what they read
/// before answering -- the ledger alone, or the ledger with every provider
/// report brought current -- and not in what they say about an account,
/// because a second projection of a subscription is a second thing to be wrong
/// about it.
pub(crate) fn document(
    scope: &PoolScope,
    entries: &[SubscriptionEntry],
    errors: Vec<Failure>,
) -> Value {
    let mut errors: Vec<Value> = errors
        .into_iter()
        .map(|error| failure_json(&error))
        .collect();
    let observed_at_ms = now_ms();
    // One row shape for every audience. The rows an agent may see are fewer
    // than the operator's, and that is the whole of the difference: a console
    // and an agent reading the same account read the same fields about it,
    // because a second projection is a second thing to be wrong.
    let rows = entries
        .iter()
        .map(|entry| {
            let recorded = usage::usage_for(&entry.id);
            let windows = usage::plan_windows(recorded.as_ref());
            if !errors.iter().any(|error| {
                error
                    .pointer("/context/subscription")
                    .and_then(Value::as_str)
                    == Some(entry.id.as_str())
            }) {
                if let Some(error) =
                    reading_error(entry, recorded.as_ref(), &windows, observed_at_ms)
                {
                    errors.push(error);
                }
            }
            if entry.status == "active" && !retired(&entry.id, recorded.as_ref()) {
                if let Some(failure) =
                    crate::subscription_dispatch::sign_in::observed_failure(&entry.id)
                {
                    errors.push(failure.failure(Some(&entry.id)));
                }
            }
            let mut row = subscription_row(entry, recorded.as_ref(), windows);
            row["state"] = json!(if entry.status == "undiscovered" {
                "unknown"
            } else if retired(&entry.id, recorded.as_ref()) {
                "burnt"
            } else {
                state(recorded.as_ref(), observed_at_ms)
            });
            row["expires_at"] = expires_at(recorded.as_ref());
            row["last_redeem_error"] = last_redeem_error(recorded.as_ref(), observed_at_ms);
            row
        })
        .collect::<Vec<_>>();
    if let Some(detail) = usage::storage_error() {
        errors.push(failure_json(
            &failure::envelope(
                "brama.subscriptions.ledger",
                Code::Config,
                "subscription usage history",
                detail,
            )
            .with_context("attempted_at_ms", observed_at_ms.to_string()),
        ));
    }
    let ok = errors.is_empty();
    json!({
        "ok": ok,
        "observed_at_ms": observed_at_ms,
        "scope": scope.named(),
        "errors": errors,
        "subscriptions": rows,
    })
}

fn failure_json(failure: &Failure) -> Value {
    serde_json::from_str(&failure.to_json()).expect("Wisent failure serialization is JSON")
}

pub(crate) async fn inventory(scope: &PoolScope) -> (Vec<SubscriptionEntry>, Vec<Failure>) {
    let discovered = match scope.agent() {
        Some(agent_id) => broker::discover_subscriptions(agent_id).await,
        None => broker::list_all_subscriptions().await,
    };
    let (entries, errors) = match discovered {
        Ok(entries) => (entries, Vec::new()),
        Err(detail) => {
            let mut error = failure::envelope(
                "brama.subscriptions.discovery",
                Code::Config,
                "subscription inventory",
                detail,
            )
            .with_context("attempted_at_ms", now_ms().to_string());
            if let Some(agent_id) = scope.agent() {
                error = error.with_context("agent", agent_id);
            }
            (Vec::new(), vec![error])
        }
    };
    let mut entries: BTreeMap<_, _> = entries
        .into_iter()
        .map(|entry| (entry.id.clone(), entry))
        .collect();
    // Only the deployment scope can include historical accounts. A scoped read
    // must never widen an agent's ownership from a shared usage ledger.
    if scope.agent().is_none() {
        for (id, recorded) in usage::recorded_subscriptions() {
            entries
                .entry(id.clone())
                .or_insert_with(|| SubscriptionEntry {
                    status: if retired(&id, Some(&recorded)) {
                        "retired"
                    } else {
                        "undiscovered"
                    }
                    .into(),
                    id,
                    provider: named(&recorded.provider),
                    label: None,
                    login_item: None,
                });
        }
    }
    (entries.into_values().collect(), errors)
}

/// Why one row of the document is not a healthy answer, in the words of
/// whatever produced the refusal.
///
/// A subscription the ledger remembers but the vault no longer lists is its own
/// case: nothing can confirm its credential, which is a different problem from a
/// grant that was refused.
fn reading_error(
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
    if entry.status != "active" || retired(&entry.id, recorded) {
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
