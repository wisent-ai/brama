//! The last two facts in a readiness answer and the verdict they produce: the
//! subscription accounts the vault holds but no agent can route to, and whether
//! this process can carry traffic at all.

use serde_json::{json, Value};

use super::ReadinessReport;

/// Everything the sweep established, so the verdict below is a reading of
/// facts rather than a second sweep.
pub(super) struct ServiceFacts {
    pub(super) providers: Vec<String>,
    pub(super) provider_inventory_error: Option<String>,
    pub(super) signing_inventory_error: Option<String>,
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
    let mut inventory_errors = Vec::new();
    if let Some(detail) = &facts.provider_inventory_error {
        inventory_errors.push(json!({
            "operation": "configured_provider_capabilities", "detail": detail,
        }));
    }
    if let Some(detail) = &facts.signing_inventory_error {
        inventory_errors.push(json!({
            "operation": "configured_request_sign_agents", "detail": detail,
        }));
    }
    if facts.standalone {
        if let Err(detail) = crate::gateway::broker::local_provider_names() {
            inventory_errors.push(json!({"operation": "local_provider_names", "detail": detail}));
        }
    }
    for checked in &facts.checked {
        if let Some(detail) = checked["reason"].as_str() {
            inventory_errors.push(json!({
                "operation": "provider_credential",
                "provider": checked["provider"],
                "detail": detail,
            }));
        }
    }
    // A subscription item that loses its `brama:subscription` mark disappears
    // from discovery. The broker reports those explicitly without treating
    // unrelated vault entries as subscription accounts.
    let untagged: Vec<Value> = if facts.standalone {
        Vec::new()
    } else {
        let accounts = match crate::gateway::broker::list_unroutable_accounts().await {
            Ok(accounts) => accounts,
            Err(detail) => {
                inventory_errors
                    .push(json!({"operation": "list_unroutable_accounts", "detail": detail}));
                Vec::new()
            }
        };
        accounts
            .into_iter()
            .map(|account| {
                let (provider_str, reason) = match (&account.provider, &account.id) {
                    (Some(provider), Some(id)) => {
                        let refusal = crate::subscription_dispatch::dispatch::no_active_credential_summary(provider);
                        let admission = match &account.account {
                            Some(_) => format!(
                                "the next maintenance pass restores the mark and schedules its sign-in, and `brama subscription sign-in {provider} --by weles --subscription-id {id} --reason <why>` does so now"
                            ),
                            None => "it names no provider account (brama:account:), so no sign-in can be resolved for it until the account is recorded on the item".to_string(),
                        };
                        (provider.clone(), format!(
                            "the vault holds this account and its item carries no 'brama:subscription' mark, \
                             so subscription discovery cannot see it and no caller can route to it; \
                             every request for this provider answers '{refusal}'; {admission}"
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

    // An account the vault holds and this process was never started with: the
    // runtime policy, generated at install, does not name it. A readiness
    // answer that cannot say this reports none of a gateway's paid accounts
    // while every request is refused with `no working subscription model for
    // signed agent`.
    let mut untagged = untagged;
    let mut unnamed = 0usize;
    if !facts.standalone {
        let entries = match crate::gateway::broker::policy_unnamed_subscriptions().await {
            Ok(entries) => entries,
            Err(detail) => {
                inventory_errors
                    .push(json!({"operation": "policy_unnamed_subscriptions", "detail": detail}));
                Vec::new()
            }
        };
        for entry in entries {
            untagged.push(json!({
                "id": entry.id,
                "provider": entry.provider,
                "routable": false,
                "reason": "the vault holds this subscription and this process was not started \
                    with it: the runtime policy is generated from the vault's tags when the \
                    release is installed on this host, so an account added since that install \
                    is in neither the policy nor the boot catalogue; install this release again \
                    on this host to serve it",
            }));
            unnamed += 1;
        }
    }

    // Deployment readiness answers whether this process can carry traffic, not
    // whether every account it can see is healthy. Requiring every subscription
    // to redeem made a repaired release impossible to promote: only that release
    // could run the renewal, while the rollout waited for renewal to finish.
    // Keep the full account verdict in this same report as `degraded`; a single
    // broken account remains visible without taking working routes offline.
    let serving = facts.provider_available || facts.subscription_available;
    // The same deadlock, one step further out: with EVERY credential dead no
    // release can be ready, so the candidate carrying the sign-in fix is
    // quarantined for losing readiness - and the sign-in that repairs the
    // credentials is a command of the release that cannot be promoted. A
    // configured gateway with no live
    // credential is not a broken deployment; it is a deployment waiting for a
    // sign-in, and it must be allowed to exist in order to run one. It answers
    // `ready: false` and `operator_action_required`, and `/readyz` answers 200
    // so a rollout can replace the build that cannot repair itself.
    let repairable = !facts.providers.is_empty() || facts.active_subscriptions > 0;
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
        && blocked.is_empty()
        && inventory_errors.is_empty();
    let status = if serving || repairable {
        axum::http::StatusCode::OK
    } else {
        axum::http::StatusCode::SERVICE_UNAVAILABLE
    };
    let reason = if facts.providers.is_empty()
        && facts.active_subscriptions == 0
        && untagged.is_empty()
    {
        "no provider capability or subscription is configured"
    } else if !serving {
        "no configured direct provider or subscription route can obtain a credential; this \
         deployment serves no traffic and is waiting for a sign-in, which is why it is \
         installable rather than refused"
    } else if !facts.denied.is_empty() {
        "traffic can be served, but a configured direct-provider credential could not be obtained"
    } else if !facts.unredeemable.is_empty() {
        "traffic can be served, but an active subscription credential could not be redeemed"
    } else if unnamed > 0 {
        "traffic can be served, but the vault holds a subscription this process was not started \
         with: the runtime policy does not name it"
    } else if !untagged.is_empty() {
        "traffic can be served, but the vault holds a subscription account with incomplete routing metadata"
    } else if !facts.unroutable.is_empty() {
        "traffic can be served, but at least one active subscription contributes no model"
    } else if !blocked.is_empty() {
        "traffic can be served, but at least one account cannot be signed in by this gateway"
    } else if !inventory_errors.is_empty() {
        "traffic can be served, but credential inventory is incomplete"
    } else {
        "every configured provider credential was obtained, every active subscription redeemed, and every active subscription account is routable"
    };
    let reason = if inventory_errors.is_empty() {
        reason.to_owned()
    } else {
        format!(
            "{reason}; credential inventory could not be fully read: {}",
            inventory_errors
                .iter()
                .map(|error| format!(
                    "{}: {}",
                    error["operation"].as_str().expect("operation"),
                    error["detail"].as_str().expect("detail")
                ))
                .collect::<Vec<_>>()
                .join("; ")
        )
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
            "inventory_errors": inventory_errors,
            "automatic_sign_in": json!({
                "host": placement.to_json(),
                "blocked": blocked,
            }),
            "operator_action_required": !healthy,
            "build": crate::build_info::current(),
        }),
    }
}
