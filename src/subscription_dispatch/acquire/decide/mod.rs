//! Whether to buy: every fact the purchase depends on, read at the moment
//! of the decision, and the refusal that names the one that stopped it.

mod plan;
pub(super) mod pool_state;

use serde_json::{json, Value};

use super::{AcquireOptions, Trigger, FAILED, REFUSED, REQUESTED};
use pool_state::Standing;

/// What the decision found when it chose to buy.
pub(super) struct Shortage {
    /// Which shortage it is: `spent` (every account spent its plan),
    /// `sessions_full` (every usable subscription carries the operator's
    /// limit of sessions) or `unusable` (no account can serve at all).
    pub kind: &'static str,
    /// The shortage as a sentence, with its counts.
    pub why: String,
    pub cap: u64,
    pub accounts: Vec<String>,
    pub standings: Vec<Value>,
    pub plan_tier: String,
}

/// Read the cap, the accounts, each account's plan, the leases and the plan
/// tier. `Err` is the refusal that stopped the purchase.
pub(super) async fn decide(options: &AcquireOptions) -> Result<Shortage, Box<Value>> {
    let provider = options.provider.as_str();
    let refuse =
        |code: &str, detail: String, facts: Value| Box::new(refusal(options, code, detail, facts));
    let cap = super::declaration::accounts_cap(provider).map_err(|detail| {
        refuse(
            "account_cap_undeclared",
            format!("{detail}, so none is bought"),
            json!({}),
        )
    })?;
    if options.trigger != Trigger::Operator {
        if let Some(previous) = unresolved_attempt(provider) {
            return Err(refuse(
                "previous_acquisition_unresolved",
                format!(
                    "the last acquisition of a {provider} account (at {}) ended {}: {}; a \
                     maintenance pass or a full pool of sessions buys nothing until `brama \
                     subscription acquire {provider} --reason <why>` is run again",
                    previous["at"], previous["result"], previous["detail"]
                ),
                json!({"previous": previous}),
            ));
        }
    }
    let members = pool_state::members(provider)
        .await
        .map_err(|detail| refuse("inventory_unreadable", detail, json!({})))?;
    let accounts: Vec<String> = pool_state::accounts(&members)
        .iter()
        .map(ToString::to_string)
        .collect();
    let count = u64::try_from(accounts.len()).expect("an account count fits in u64");
    if count >= cap {
        return Err(refuse(
            "account_cap_reached",
            format!(
                "the pool already holds {count} {provider} accounts and the operator allows at \
                 most {cap}: {}",
                accounts.join(", ")
            ),
            json!({"cap": cap, "accounts": accounts}),
        ));
    }
    let mut spent = Vec::new();
    let mut standings = Vec::new();
    let mut stops = Vec::new();
    for member in &members {
        if super::declaration::provider(provider).is_ok_and(|declared| declared.resets.is_some()) {
            let reset = super::resets::redeem::run(
                provider,
                &member.id,
                "use eligible saved capacity before buying another account",
                true,
            )
            .await
            .map_err(|detail| {
                refuse(
                    "saved_capacity_unconfirmed",
                    format!(
                        "{}: {detail}; no account is bought while saved capacity is unconfirmed",
                        member.id
                    ),
                    json!({"member": member.id}),
                )
            })?;
            if reset["ok"].as_bool() != Some(true) {
                return Err(refuse("saved_capacity_unconfirmed",
                    format!("{}: saved-capacity maintenance did not confirm its outcome; no account is bought", member.id),
                    json!({"member": member.id, "reset": reset})));
            }
        }
        let row = match pool_state::standing(member, provider).await {
            Standing::Spent { until_ms } => {
                spent.push(member.id.clone());
                json!({"id": member.id, "account": member.account, "standing": "spent", "until_ms": until_ms})
            }
            Standing::Available { used_fraction } => {
                stops.push(match used_fraction {
                    Some(fraction) => format!(
                        "{} still has plan left (its most spent window is {fraction:.2} used)",
                        member.id
                    ),
                    None => format!("{} still has plan left", member.id),
                });
                json!({"id": member.id, "account": member.account, "standing": "available",
                    "used_fraction": used_fraction})
            }
            Standing::Unread { detail } => json!({"id": member.id, "account": member.account,
                "standing": "unread", "detail": detail}),
        };
        standings.push(row);
    }
    let facts = || json!({"cap": cap, "accounts": accounts, "standings": standings});
    // A pool full of sessions is a shortage of accounts, not of plan: the
    // operator's limit of sessions per subscription is reached on every
    // usable member, and the standings are recorded for the verdict without
    // stopping it. Every other trigger buys when no account has plan left:
    // each has spent it, or cannot be used at all (unread, or no account).
    let (kind, why) = if options.trigger == Trigger::SessionsFull {
        match super::super::leases::sessions_shortage(provider).await {
            Ok(Some(full)) => (
                "sessions_full",
                format!(
                    "every usable {provider} subscription carries {} sessions, the operator's limit",
                    full["limit"]
                ),
            ),
            Ok(None) => {
                return Err(refuse(
                    "sessions_not_full",
                    format!(
                        "not every usable {provider} subscription carries the operator's limit of sessions, so none is bought"
                    ),
                    facts(),
                ))
            }
            Err(detail) => return Err(refuse("leases_unreadable", detail, json!({}))),
        }
    } else if !stops.is_empty() {
        return Err(refuse(
            "pool_not_spent",
            format!(
                "not every {provider} account has spent its plan, so none is bought: {}",
                stops.join("; ")
            ),
            facts(),
        ));
    } else if !members.is_empty() && spent.len() == members.len() {
        (
            "spent",
            format!(
                "every one of the {} {provider} accounts has spent its plan",
                members.len()
            ),
        )
    } else {
        (
            "unusable",
            format!(
                "no {provider} account can serve: of {} accounts, {} spent and the rest cannot be read",
                members.len(),
                spent.len()
            ),
        )
    };
    let plan_tier = plan::pool_tier(provider, &members)
        .await
        .map_err(|detail| refuse("plan_unstated", detail, facts()))?;
    Ok(Shortage {
        kind,
        why,
        cap,
        accounts,
        standings,
        plan_tier,
    })
}

/// The newest attempt for `provider` when it failed or never reported how it
/// ended: the state in which a maintenance pass stops buying.
fn unresolved_attempt(provider: &str) -> Option<Value> {
    crate::journal::latest_subscription_acquisition(provider).filter(|previous| {
        let result = previous["result"].as_str();
        result == Some(FAILED) || result == Some(REQUESTED)
    })
}

/// A decision not to buy. Refusals are not journaled: nothing was asked of
/// anyone, and a pass that refuses every minute would bury the purchases.
pub(super) fn refusal(options: &AcquireOptions, code: &str, detail: String, facts: Value) -> Value {
    let mut verdict = json!({
        "provider": options.provider,
        "reason": options.reason,
        "trigger": options.trigger.name(),
        "result": REFUSED,
        "code": code,
        "detail": detail,
    });
    if let (Some(verdict), Some(facts)) = (verdict.as_object_mut(), facts.as_object()) {
        for (key, value) in facts {
            verdict.insert(key.clone(), value.clone());
        }
    }
    verdict
}
