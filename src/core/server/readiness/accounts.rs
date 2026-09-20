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
    // A subscription item that loses its `brama:subscription` mark disappears
    // from discovery. The broker reports those explicitly without treating
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
                            "the vault holds this account and its item carries no 'brama:subscription' mark, \
                             so subscription discovery cannot see it and no caller can route to it; \
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

    // An account the vault holds and this process was never started with: the
    // runtime policy, generated at install, does not name it. Until
    // 2026-09-20 the readiness answer could not say this, and a gateway that
    // held five paid accounts reported none of them and refused every request
    // with `no working subscription model for signed agent`.
    let mut untagged = untagged;
    let mut unnamed = 0usize;
    if !facts.standalone {
        for entry in crate::gateway::broker::policy_unnamed_subscriptions().await {
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
    // The same deadlock, one step further out, and the state this fleet was in
    // on 2026-09-19: EVERY credential was dead, so no release could be ready,
    // so the candidate carrying the sign-in fix was quarantined for losing
    // readiness - and the sign-in that repairs the credentials is a command of
    // the release that could not be promoted. A configured gateway with no live
    // credential is not a broken deployment; it is a deployment waiting for a
    // sign-in, and it must be allowed to exist in order to run one. It answers
    // `ready: false` and `operator_action_required`, and `/readyz` answers 200
    // so a rollout can replace the build that cannot repair itself.
    let repairable = !facts.providers.is_empty() || facts.active_subscriptions > usize::MIN;
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
    let status = if serving || repairable {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn facts(providers: Vec<String>, provider_available: bool) -> ServiceFacts {
        ServiceFacts {
            providers,
            checked: Vec::new(),
            denied: Vec::new(),
            provider_available,
            standalone: true,
            routable: Vec::new(),
            unroutable: Vec::new(),
            active_subscriptions: usize::MIN,
            subscriptions: Vec::new(),
            unredeemable: Vec::new(),
            sign_in_blocked: std::collections::BTreeMap::new(),
            subscription_available: false,
        }
    }

    /// On 2026-09-19 every subscription credential on charless-mac-mini was
    /// dead, so `/readyz` answered 503, so Stado quarantined brama 0.4.39 for
    /// losing readiness - and the sign-in that repairs those credentials is a
    /// command of the release that could therefore never be promoted. A
    /// configured gateway with no live credential serves nothing and is still
    /// installable.
    #[tokio::test]
    async fn a_configured_gateway_with_no_live_credential_is_installable() {
        let report = verdict(facts(vec!["openai".to_owned()], false)).await;
        assert_eq!(report.status, axum::http::StatusCode::OK);
        assert_eq!(report.body["ready"], false);
        assert_eq!(report.body["operator_action_required"], true);
        assert!(
            report.body["reason"]
                .as_str()
                .unwrap_or_default()
                .contains("waiting for a sign-in"),
            "{}",
            report.body
        );
    }

    /// A process with nothing configured is not a deployment waiting for a
    /// sign-in; there is nothing to sign in, and it stays refused.
    #[tokio::test]
    async fn a_gateway_with_nothing_configured_is_refused() {
        let report = verdict(facts(Vec::new(), false)).await;
        assert_eq!(
            report.status,
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            "{}",
            report.body
        );
        assert_eq!(report.body["ready"], false);
    }

    /// A live credential still reads as serving, and as healthy.
    #[tokio::test]
    async fn a_gateway_holding_a_credential_serves() {
        let report = verdict(facts(vec!["openai".to_owned()], true)).await;
        assert_eq!(report.status, axum::http::StatusCode::OK);
        assert_eq!(report.body["ready"], true);
    }
}
