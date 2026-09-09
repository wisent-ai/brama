//! Does the credential chain actually work right now?
//!
//! One sweep: a capability per configured provider, every active subscription
//! an agent can see, and one redemption per subscription at the boundary where
//! a model request redeems it. Provider, agent and subscription groups are
//! independent, so each runs concurrently and one slow provider no longer
//! delays every check behind it.

use futures_util::future::join_all;
use serde_json::json;

use super::accounts::{self, ServiceFacts};
use super::ReadinessReport;

/// What `/readyz` says about a provider whose active subscription contributed
/// no model, given the refusals the discovery sweep recorded for it.
///
/// "No model discovered" on its own cannot distinguish a provider that
/// answered with nothing from a credential this gateway could not derive a key
/// from and therefore never asked with — and those two have different owners.
/// The sweep already knew which and kept it to itself: `discover_models`
/// records every per-subscription refusal and then drops the list unless the
/// pool ended with no models at all, so one working subscription silenced the
/// reason every other one failed.
///
/// Measured on charless-mac-mini on 2026-09-02: claude-code and kimi both
/// redeemed and both discovered nothing while codex, in the same sweep on the
/// same host, discovered five. Separating "the provider answered with nothing"
/// from "we never asked" took reading this crate's branches and counting
/// `static_models` entries, because no surface would say it. Every
/// subscription provider in the registry carries a static model list, so an
/// empty result cannot come from the provider's own answer at all — only from
/// a refusal reached before that list is ever consulted.
pub fn unroutable_reason(refusals: &[String]) -> String {
    const HEADLINE: &str = "active subscription, no model discovered";
    if refusals.is_empty() {
        return HEADLINE.to_string();
    }
    format!("{HEADLINE} — {}", refusals.join("; "))
}

pub(super) async fn calculate_readiness() -> ReadinessReport {
    let providers: Vec<String> = {
        let mut names: Vec<String> = crate::gateway::broker::configured_provider_capabilities()
            .into_iter()
            .collect();
        names.sort();
        names
    };

    let provider_results = join_all(providers.iter().map(|provider| async {
        let obtained = crate::gateway::broker::provider_credential(provider)
            .await
            .is_some();
        (provider.clone(), obtained)
    }))
    .await;
    let provider_available = provider_results.iter().any(|(_, obtained)| *obtained);
    let mut checked = Vec::with_capacity(provider_results.len());
    let mut denied = Vec::new();
    for (provider, obtained) in provider_results {
        if !obtained {
            denied.push(provider.clone());
        }
        checked.push(json!({ "provider": provider, "credential": obtained }));
    }

    // Obtaining a credential is only the first half. A subscription whose model
    // discovery yields nothing is active, its credential redeems, and it still
    // cannot be routed to. Every active subscription is collected once,
    // whichever agents can see it.
    let standalone = crate::gateway::broker::local_provider_credentials_enabled();
    let agents = if standalone {
        Vec::new()
    } else {
        crate::gateway::broker::configured_request_sign_agents()
    };
    let agent_results = join_all(agents.into_iter().map(|agent| async move {
        let subscriptions = crate::gateway::broker::list_subscriptions(&agent).await;
        let models =
            crate::subscription_dispatch::dispatch::registry_models_for_agent(&agent).await;
        (agent, subscriptions, models)
    }))
    .await;

    let mut routable = Vec::new();
    let mut unroutable = Vec::new();
    let mut active = std::collections::BTreeMap::<String, String>::new();
    // Recorded authentication operations, not a prediction derived from tags.
    let mut sign_in_blocked = std::collections::BTreeMap::<
        String,
        (String, crate::subscription_dispatch::sign_in::Blocked),
    >::new();
    let mut model_providers = std::collections::BTreeSet::<String>::new();
    for (agent, entries, models) in agent_results {
        let mut subscribed = Vec::new();
        for entry in entries {
            if entry.status != "active" {
                continue;
            }
            if !crate::journal::is_retired(&entry.id) {
                if let Some(failure) =
                    crate::subscription_dispatch::sign_in::observed_failure(&entry.id)
                {
                    sign_in_blocked
                        .entry(entry.id.clone())
                        .or_insert((entry.provider.clone(), failure));
                }
            }
            let provider = entry.provider.trim().to_string();
            subscribed.push(provider.clone());
            active.entry(entry.id).or_insert(provider);
        }
        match models {
            Ok(models) => {
                let mut per_provider = std::collections::BTreeMap::<String, usize>::new();
                for model in &models {
                    let provider =
                        crate::subscription_dispatch::dispatch::provider_for(&model.route_id)
                            .unwrap_or("unattributed")
                            .to_string();
                    model_providers.insert(provider.clone());
                    *per_provider.entry(provider).or_default() += usize::from(true);
                }
                for provider in &subscribed {
                    if !per_provider.contains_key(provider) {
                        // Why, when the sweep recorded one. "No model
                        // discovered" alone cannot distinguish a provider that
                        // answered with nothing from a credential this gateway
                        // could not derive a key from and so never asked with,
                        // and those have different owners.
                        let refusals: Vec<String> = active
                            .iter()
                            .filter(|(_, subscribed_provider)| *subscribed_provider == provider)
                            .filter_map(|(subscription, subscribed_provider)| {
                                crate::subscription_dispatch::dispatch::discovery_failure(
                                    subscribed_provider,
                                    subscription,
                                )
                                .map(|why| format!("{subscription}: {why}"))
                            })
                            .collect();
                        let reason = unroutable_reason(&refusals);
                        unroutable.push(json!({
                            "agent": agent,
                            "provider": provider,
                            "reason": reason,
                        }));
                    }
                }
                routable.push(json!({
                    "agent": agent,
                    "models": models.len(),
                    "by_provider": per_provider,
                }));
            }
            Err(error) => {
                unroutable.push(json!({
                    "agent": agent,
                    "provider": "all",
                    "reason": error,
                }));
            }
        }
    }

    // The act itself: one redemption per active subscription, at the boundary
    // where a model request redeems it.
    let subscription_results = join_all(active.iter().map(|(subscription, provider)| async {
        let refusal = crate::subscription_dispatch::dispatch::probe_subscription_redemption(
            subscription,
            provider,
        )
        .await
        .err();
        (subscription.clone(), provider.clone(), refusal)
    }))
    .await;
    let mut subscriptions = Vec::with_capacity(subscription_results.len());
    let mut unredeemable = Vec::new();
    let mut subscription_available = false;
    for (subscription, provider, refusal) in subscription_results {
        if refusal.is_some() {
            unredeemable.push(subscription.clone());
        } else if model_providers.contains(&provider) {
            subscription_available = true;
        }
        subscriptions.push(json!({
            "id": subscription,
            "provider": provider,
            "redeemable": refusal.is_none(),
            "reason": refusal.unwrap_or_else(|| "the credential redeemed".to_string()),
        }));
    }

    accounts::verdict(ServiceFacts {
        providers,
        checked,
        denied,
        provider_available,
        standalone,
        routable,
        unroutable,
        active_subscriptions: active.len(),
        subscriptions,
        unredeemable,
        sign_in_blocked,
        subscription_available,
    })
    .await
}
