//! Whether this gateway has a model to serve with, read from the ledger each
//! time `/readyz` is asked: `model.state` is `available` while some signing
//! agent's pool holds a member that can be served (live, or expired or
//! unchecked and so renewable when a request redeems it), `none` (with every
//! member's refusal and the sign-in that renews it in `model.reason`) while
//! every member needs a new sign-in, `unknown` (with the failed read) when the
//! pool cannot be read, and `local` on a gateway serving its own provider
//! credentials. `/readyz` answers 503 while it says `none` or `unknown`, and a
//! client reading it sends nothing, naming the reason.

use serde_json::{json, Map, Value};

/// The `model` document `/readyz` carries.
pub(in crate::core::server::readiness) fn document() -> Value {
    if crate::gateway::broker::local_provider_credentials_enabled() {
        return json!({
            "state": "local",
            "reason": "this gateway serves its own provider credentials, not a subscription pool",
        });
    }
    let agents = match crate::gateway::broker::configured_request_sign_agents() {
        Ok(agents) => agents,
        Err(cause) => {
            let reason = format!("the signing agents cannot be read: {cause}");
            return json!({ "state": "unknown", "reason": reason });
        }
    };
    if agents.is_empty() {
        let reason = "no signing agent is configured, so no subscription pool serves";
        return json!({ "state": "none", "reason": reason });
    }
    let mut refusals = Vec::with_capacity(agents.len());
    for agent in &agents {
        match crate::subscription_dispatch::dispatch::live_pool_refusal(agent) {
            Ok(None) => return json!({ "state": "available", "reason": Value::Null }),
            Ok(Some(refusal)) => refusals.push(format!("{agent}: {refusal}")),
            Err(refused) => {
                return json!({
                    "state": "unknown",
                    "reason": format!("agent {agent}'s pool cannot be read: {}", refused.message),
                })
            }
        }
    }
    json!({ "state": "none", "reason": refusals.join(" | ") })
}

/// Mark a readiness answer unready for `cause`, keeping what it already said.
/// A report that was ready gives way to `cause`; one already unready for a
/// reason in words keeps it before `cause` after ` | `; any other shape of its
/// own `ready` and `reason` is named as it stands before `cause`, so no earlier
/// state is dropped without being said.
pub(in crate::core::server::readiness) fn unready(object: &mut Map<String, Value>, cause: &str) {
    let reason = match (object.get("ready"), object.get("reason")) {
        (Some(Value::Bool(true)), _) => cause.to_string(),
        (Some(Value::Bool(false)), Some(Value::String(held))) => format!("{held} | {cause}"),
        (ready, held) => format!(
            "the report itself read ready {} and reason {} | {cause}",
            stated(ready),
            stated(held)
        ),
    };
    object.insert("ready".to_string(), Value::Bool(false));
    object.insert("reason".to_string(), Value::String(reason));
}

/// A field of the report as it stands, or that it is absent.
fn stated(field: Option<&Value>) -> String {
    match field {
        Some(value) => value.to_string(),
        None => "absent".to_string(),
    }
}
