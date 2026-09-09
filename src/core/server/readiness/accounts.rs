//! The last two facts in a readiness answer and the verdict they produce: the
//! subscription accounts the vault holds but no agent can route to, and whether
//! this process can carry traffic at all.

use serde_json::{json, Value};

use super::ReadinessReport;

/// Everything the sweep established, so the verdict below is a reading of
/// facts rather than a second sweep.
pub(super) struct ServiceFacts {
    pub(super) providers: Vec<String>,
    pub(super) checked: Vec<Value>,
    pub(super) denied: Vec<String>,
    pub(super) provider_available: bool,
    pub(super) standalone: bool,
    pub(super) routable: Vec<Value>,
    pub(super) unroutable: Vec<Value>,
    pub(super) active_subscriptions: usize,
    pub(super) subscriptions: Vec<Value>,
    pub(super) unredeemable: Vec<String>,
    /// Accounts this gateway cannot sign in by itself, and why. Keyed by
    /// subscription id so the reason travels with the account it is about.
    pub(super) sign_in_blocked: std::collections::BTreeMap<
        String,
        (String, crate::subscription_dispatch::sign_in::Blocked),
    >,
    pub(super) subscription_available: bool,
}

pub(super) async fn verdict(facts: ServiceFacts) -> ReadinessReport {
    // A subscription item that loses every `brama:agent:` tag disappears from
    // normal discovery. The broker reports those explicitly without treating
    // unrelated vault entries as subscription accounts.
    let untagged: Vec<Value> = if facts.standalone {
        Vec::new()
    } else {
        crate::gateway::broker::list_unroutable_accounts()
            .await
            .into_iter()
            .map(|account| {
                let (provider_str, reason) = match (&account.provider, &account.id) {
                    (Some(provider), Some(_id)) => {
                        let refusal = crate::subscription_dispatch::dispatch::no_active_credential_summary(provider);
                        (provider.clone(), format!(
                            "the vault holds this account and its item carries no 'brama:agent:' tag, \
                             so subscription discovery cannot see it and no agent can route to it; \
                             every request for this provider answers '{refusal}'"
                        ))
                    }
                    (Some(provider), None) => {
                        let refusal = crate::subscription_dispatch::dispatch::no_active_credential_summary(provider);
                        (provider.clone(), format!(
                            "the vault holds this account; its item carries 'brama:provider:' but no 'brama:id:' tag; \
                             subscription discovery cannot route to it without both tags; \
                             every request for this provider would answer '{refusal}'"
                        ))
                    }
                    (None, Some(_id)) => {
                        ("unknown".to_string(),
                         "the vault holds this account; its item carries 'brama:id:' but no 'brama:provider:' tag; \
                          subscription discovery cannot route to it without both tags; \
                          operator: add the missing 'brama:provider:' tag or remove this item".to_string())
                    }
                    (None, None) => {
                        ("unknown".to_string(),
                         "the vault holds this subscription account, but its item carries no 'brama:id:' or 'brama:provider:' tags; \
                          subscription discovery cannot route to it; \
                          operator: restore both tags or remove this item from the vault".to_string())
                    }
                };
                json!({
                    "id": account.id,
                    "provider": &provider_str,
                    "item": account.item,
                    "routable": false,
                    "reason": reason,
                })
            })
            .collect()
    };

    // Deployment readiness answers whether this process can carry traffic, not
    // whether every account it can see is healthy. Requiring every subscription
    // to redeem made a repaired release impossible to promote: only that release
    // could run the renewal, while the rollout waited for renewal to finish.
    // Keep the full account verdict in this same report as `degraded`; a single
    // broken account remains visible without taking working routes offline.
    let serving = facts.provider_available || facts.subscription_available;
    // What the automatic loop can and cannot repair, which is a different
    // question from whether a credential is currently good: a deployment can
    // hold nothing but healthy grants and still be unable to replace any of
    // them, and that is the state that ended in an outage.
    let placement = super::placement::placement();
    let mut blocked: Vec<Value> = facts
        .sign_in_blocked
        .iter()
        .map(|(id, (provider, blocked))| {
            let mut row = blocked.to_json();
            row["id"] = json!(id);
            row["provider"] = json!(provider);
            row
        })
        .collect();
    if let Some(drift) = placement.blocked() {
        blocked.push(drift.to_json());
    }
    let healthy = serving
        && facts.denied.is_empty()
        && facts.unredeemable.is_empty()
        && untagged.is_empty()
        && facts.unroutable.is_empty()
        && blocked.is_empty();
    let status = if serving {
        axum::http::StatusCode::OK
    } else {
        axum::http::StatusCode::SERVICE_UNAVAILABLE
    };
    let reason = if facts.providers.is_empty()
        && facts.active_subscriptions == usize::MIN
        && untagged.is_empty()
    {
        "no provider capability or subscription is configured"
    } else if !serving {
        "no configured direct provider or subscription route can obtain a credential"
    } else if !facts.denied.is_empty() {
        "traffic can be served, but a configured direct-provider credential could not be obtained"
    } else if !facts.unredeemable.is_empty() {
        "traffic can be served, but an active subscription credential could not be redeemed"
    } else if !untagged.is_empty() {
        "traffic can be served, but the vault holds a subscription account with no agent route"
    } else if !facts.unroutable.is_empty() {
        "traffic can be served, but at least one active subscription contributes no model"
    } else if !blocked.is_empty() {
        "traffic can be served, but at least one account cannot be signed in by this gateway"
    } else {
        "every configured provider credential was obtained, every active subscription redeemed, and every active subscription account is routable"
    };
    ReadinessReport {
        status,
        body: json!({
            "ready": serving,
            "degraded": !healthy,
            "reason": reason,
            "providers": facts.checked,
            "denied": facts.denied,
            "routing": facts.routable,
            "unroutable": facts.unroutable,
            "subscriptions": facts.subscriptions,
            "unredeemable": facts.unredeemable,
            "unroutable_accounts": untagged,
            "automatic_sign_in": json!({
                "host": placement.to_json(),
                "blocked": blocked,
            }),
            "operator_action_required": !healthy,
            "build": crate::build_info::current(),
        }),
    }
}
