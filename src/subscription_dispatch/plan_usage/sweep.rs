//! One pass that reads every plan reading that is due.
//!
//! Nothing here talks to a provider directly: this is the rule that a
//! subscription two agents share is still one account with one plan and is
//! visited once, and that one refused account does not stop the accounts after
//! it. Whether a visit becomes a request is the cache window's decision, not
//! this file's. Each maintenance pass of the serving process runs it once,
//! and so does `brama maintain`.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use tracing::{info, warn};

use super::refresh;
use crate::gateway::broker;
use crate::subscription_dispatch::usage;

/// Read every active subscription whose reading is due ([`crate::subscription_dispatch::usage::plan_usage_due`]),
/// and report each read that failed with its error envelope.
pub(crate) async fn sweep() -> Value {
    let mut seen = BTreeSet::new();
    let mut read = Vec::new();
    let mut failed = Vec::new();
    // Members whose grant the provider disowned: their plan cannot be read
    // with that grant and its refresh is not asked again until a sign-in, so
    // they are listed with the ledger's cause instead of failing once more.
    let mut awaiting_sign_in = Vec::new();
    let agents = match broker::configured_request_sign_agents() {
        Ok(agents) => agents,
        Err(error) => {
            failed.push(json!({"operation": "configured_request_sign_agents", "error": error}));
            Vec::new()
        }
    };
    for agent in agents {
        for entry in broker::list_subscriptions(&agent).await {
            if entry.status != "active" {
                continue;
            }
            match crate::journal::is_retired(&entry.id) {
                Ok(true) => continue,
                Ok(false) => {}
                Err(error) => {
                    failed.push(json!({
                        "subscription": entry.id,
                        "provider": entry.provider,
                        "error": error,
                    }));
                    continue;
                }
            }
            // A subscription shared by two agents is one account with one plan,
            // and reading it twice would ask one provider the same question
            // twice in one second.
            if !seen.insert(entry.id.clone()) {
                continue;
            }
            if !usage::plan_usage_due(&entry.id) {
                continue;
            }
            if let Some(cause) = usage::awaiting_sign_in_cause(&entry.id) {
                awaiting_sign_in.push(json!({
                    "subscription": entry.id,
                    "provider": entry.provider,
                    "cause": cause,
                }));
                continue;
            }
            read.push(entry.id.clone());
            if let Err(error) = refresh(&entry.id, &entry.provider).await {
                warn!(
                    event = "plan_usage_refresh_failed",
                    subscription = %entry.id,
                    provider = %entry.provider,
                    envelope = %error.to_json(),
                    "reading one provider usage report failed; the remaining subscriptions are unaffected"
                );
                failed.push(json!({
                    "subscription": entry.id,
                    "provider": entry.provider,
                    "error": error.to_json(),
                }));
            }
        }
    }
    info!(
        event = "plan_usage_swept",
        subscriptions = seen.len(),
        read = read.len(),
        failed = failed.len(),
        "finished one provider usage report sweep"
    );
    json!({
        "subscriptions": seen.len(),
        "read": read.len(),
        "failed": failed,
        "awaiting_sign_in": awaiting_sign_in,
    })
}
