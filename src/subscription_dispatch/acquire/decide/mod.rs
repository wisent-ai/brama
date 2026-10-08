//! Whether to buy: every fact the purchase depends on, read at the moment
//! of the decision, and the refusal that names the one that stopped it.

mod plan;
mod pool_state;

use serde_json::{json, Value};

use super::{AcquireOptions, Trigger, FAILED, REFUSED, REQUESTED};
use pool_state::Standing;

/// What the decision found when it chose to buy.
pub(super) struct Shortage {
    pub cap: u64,
    pub accounts: Vec<String>,
    pub standings: Vec<Value>,
    pub plan_tier: String,
}

/// Read the cap, the accounts, each account's plan and the plan tier.
/// `Err` is the refusal that stopped the purchase.
pub(super) async fn decide(options: &AcquireOptions) -> Result<Shortage, Box<Value>> {
    let provider = options.provider.as_str();
    let refuse =
        |code: &str, detail: String, facts: Value| Box::new(refusal(options, code, detail, facts));
    let Some(cap) = super::accounts_cap(provider) else {
        return Err(refuse(
            "account_cap_undeclared",
            format!(
                "the operator declared no cap on {provider} accounts in Brama's \
                 numeric-provenance.json, so none is bought"
            ),
            json!({}),
        ));
    };
    if options.trigger == Trigger::Maintenance {
        if let Some(previous) = unresolved_attempt(provider) {
            return Err(refuse(
                "previous_acquisition_unresolved",
                format!(
                    "the last acquisition of a {provider} account (at {}) ended {}: {}; a \
                     maintenance pass buys nothing until `brama subscription acquire {provider} \
                     --reason <why>` is run again",
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
    if members.is_empty() {
        return Err(refuse(
            "pool_empty",
            format!(
                "the pool holds no {provider} account, so no account can have spent its plan; an \
                 acquisition adds to a pool whose accounts are all spent"
            ),
            json!({"cap": cap, "accounts": accounts}),
        ));
    }
    let mut standings = Vec::new();
    let mut stops = Vec::new();
    for member in &members {
        let row = match pool_state::standing(member, provider).await {
            Standing::Spent { until_ms } => json!({"id": member.id, "account": member.account,
                "standing": "spent", "until_ms": until_ms}),
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
            Standing::Unread { detail } => {
                stops.push(format!("{} cannot be read: {detail}", member.id));
                json!({"id": member.id, "account": member.account, "standing": "unread",
                    "detail": detail})
            }
        };
        standings.push(row);
    }
    if !stops.is_empty() {
        return Err(refuse(
            "pool_not_spent",
            format!(
                "not every {provider} account has spent its plan, so none is bought: {}",
                stops.join("; ")
            ),
            json!({"cap": cap, "accounts": accounts, "standings": standings}),
        ));
    }
    let plan_tier = plan::pool_tier(provider, &members)
        .await
        .map_err(|detail| {
            refuse(
                "plan_unstated",
                detail,
                json!({"cap": cap, "accounts": accounts, "standings": standings}),
            )
        })?;
    Ok(Shortage {
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
