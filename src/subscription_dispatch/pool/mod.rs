//! What the subscription pool holds, stated from what is already recorded.
//!
//! Without this document, an empty pool shows only as `429
//! subscription_unavailable` on every `best`-aliased call, and the only way to
//! tell an exhausted plan from a disowned grant is to read the unit's log by
//! hand. The gateway knows at its first refresh sweep: the ledger carries
//! `needs_reauthorization` against the grant with the provider's own sentence
//! beside it. This document says it out loud.
//!
//! [`report`] contacts no provider and redeems no capability -- it joins the
//! deployment's subscription listing to the ledger and states what is already
//! recorded -- so it is safe to run against a gateway that is serving traffic.
//! Acting on what it says is `refresh`, which rotates grants and therefore
//! demands a reason and leaves an audit record. Neither can print credential
//! material: this reads the ledger, which has never held any.
//!
//! `scope` holds how much of the pool one proven identity may be told about,
//! `account` holds the one row shape every audience reads, and `attribution`
//! holds which provider account each row belongs to.

mod account;
mod attribution;
mod reading;
mod refresh;
mod scope;

use std::collections::BTreeMap;

use serde_json::{json, Value};
use wisent_errors::{Code, Failure};

use crate::core::failure;
use crate::gateway::broker::{self, SubscriptionEntry};
use crate::subscription_dispatch::usage;

use account::{expires_at, last_redeem_error, named, now_ms, retired, state, subscription_row};
use attribution::{accounts, capacity};
use reading::{failure_json, reading_error};

pub use account::subscription_view;
pub use attribution::capacity::{Account as AccountCapacity, Window as CapacityWindow};
pub use attribution::{record_accounts, second_factor_report};
pub use refresh::{refresh_provider, refresh_subscription, reinstate_member, ReinstateRefused};
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
    let seed_states = std::cell::OnceCell::new();
    let leases = crate::subscription_dispatch::leases::counts();
    let mut journal_unreadable = false;
    // What every account has left is read off the same observations the
    // rows are built from, so the two never disagree about a member.
    let mut observed = Vec::with_capacity(entries.len());
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
            let retirement = retired(&entry.id, recorded.as_ref());
            if let Err(cause) = &retirement {
                // Named once for the whole report: the same journal refuses
                // every row the same way.
                if !journal_unreadable {
                    journal_unreadable = true;
                    errors.push(failure_json(
                        &failure::envelope(
                            "brama.subscriptions.journal",
                            Code::Config,
                            "subscription retirement journal",
                            cause.clone(),
                        )
                        .with_context("attempted_at_ms", observed_at_ms.to_string()),
                    ));
                }
            }
            if entry.status == "active" && retirement == Ok(false) {
                if let Some(failure) =
                    crate::subscription_dispatch::sign_in::observed_failure(&entry.id)
                {
                    errors.push(failure.failure(Some(&entry.id)));
                }
            }
            let mut row = subscription_row(entry, recorded.as_ref(), windows);
            account::annotate_second_factor(&mut row, &seed_states);
            row["state"] = json!(if retirement.is_err() {
                "unknown"
            } else if retirement == Ok(true) {
                "burnt"
            } else {
                state(recorded.as_ref(), observed_at_ms)
            });
            row["expires_at"] = expires_at(recorded.as_ref());
            row["last_redeem_error"] = last_redeem_error(recorded.as_ref(), observed_at_ms);
            // The sessions the member carries, from the lease register: a
            // member no live lease names carries none. Only an unreadable
            // register is `null`, and is a failure of this report, named once.
            let live_sessions = leases
                .as_ref()
                .ok()
                .map(|counts| counts.get(&entry.id).copied().into_iter().sum::<u64>());
            row["live_sessions"] = json!(live_sessions);
            observed.push(capacity::Observed {
                entry,
                recorded,
                retired: retirement == Ok(true),
                live_sessions,
            });
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
    if let Err(detail) = &leases {
        errors.push(failure_json(
            &failure::envelope(
                "brama.subscriptions.leases",
                Code::Config,
                "subscription lease register",
                detail.clone(),
            )
            .with_context("attempted_at_ms", observed_at_ms.to_string()),
        ));
    }
    let acquisition = acquisitions(&mut errors, observed_at_ms);
    let ok = errors.is_empty();
    json!({
        "ok": ok,
        "build": crate::build_info::current(),
        "observed_at_ms": observed_at_ms,
        "scope": scope.named(),
        "errors": errors,
        "accounts": accounts(&rows, capacity::accounts(&observed, observed_at_ms)),
        "acquisition": acquisition,
        "subscriptions": rows,
        "remembered": remembered(scope, entries),
    })
}

/// The ids the usage ledger remembers that are no pool member: the vault
/// does not list them and they were never retired. They are answered apart
/// from `subscriptions`, counted nowhere, so the console can drop each with
/// `brama subscription forget`; an agent's scoped read is told none.
fn remembered(scope: &PoolScope, entries: &[SubscriptionEntry]) -> Value {
    if scope.agent().is_some() {
        return json!([]);
    }
    let members: std::collections::BTreeSet<&str> =
        entries.iter().map(|entry| entry.id.as_str()).collect();
    usage::recorded_subscriptions()
        .into_iter()
        .filter(|(id, _)| !members.contains(id.as_str()))
        .map(|(id, recorded)| json!({ "id": id, "provider": named(&recorded.provider) }))
        .collect()
}

/// The newest acquisition journaled for every provider `providers.json`
/// declares, `null` for one never bought for: what `brama subscription list`
/// prints as `last purchase` and Brama Desktop shows under its purchase
/// panel. A journal that cannot be read is one failure of the report, named
/// once, and the object is left out rather than read as "never bought".
fn acquisitions(errors: &mut Vec<Value>, observed_at_ms: i64) -> Value {
    let mut latest = serde_json::Map::new();
    for provider in crate::subscription_dispatch::acquire::declaration::declared_providers() {
        match crate::journal::latest_subscription_acquisition(provider) {
            Ok(Some(record)) => {
                latest.insert(provider.to_owned(), record);
            }
            Ok(None) => {
                latest.insert(provider.to_owned(), Value::Null);
            }
            Err(cause) => {
                errors.push(failure_json(
                    &failure::envelope(
                        "brama.subscriptions.acquisition",
                        Code::Config,
                        "subscription acquisition journal",
                        cause,
                    )
                    .with_context("attempted_at_ms", observed_at_ms.to_string()),
                ));
                return Value::Null;
            }
        }
    }
    Value::Object(latest)
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
    // Only the deployment scope can include historical accounts, and only
    // those retired, so a burnt account still says why it left. A scoped read
    // must never widen an agent's ownership from a shared usage ledger. A
    // member the usage history remembers but the vault does not list is not
    // a subscription this deployment holds, whatever id it ran under: listed
    // as `undiscovered`, every test and renamed id that ever served a request
    // stood beside the real accounts. Such a row stays in the ledger until
    // `brama subscription forget` drops it, and is not a pool member.
    if scope.agent().is_none() {
        for (id, recorded) in usage::recorded_subscriptions() {
            if entries.contains_key(&id) || retired(&id, Some(&recorded)) != Ok(true) {
                continue;
            }
            entries.insert(
                id.clone(),
                SubscriptionEntry {
                    status: "retired".into(),
                    id,
                    provider: named(&recorded.provider),
                    label: None,
                    login_item: None,
                    account: recorded
                        .discovery
                        .as_ref()
                        .map(|account| account.account.clone()),
                },
            );
        }
    }
    (entries.into_values().collect(), errors)
}
