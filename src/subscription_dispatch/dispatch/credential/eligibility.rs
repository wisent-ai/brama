//! Which of a deployment's subscriptions one call may be billed to.

use crate::gateway::broker;

use super::super::catalogue::route::provider_matches;

pub(in crate::subscription_dispatch::dispatch) fn eligible_subscription_entries(
    entries: Vec<broker::SubscriptionEntry>,
    provider: &str,
    target: Option<&crate::types::BillingTarget>,
) -> Result<Vec<broker::SubscriptionEntry>, String> {
    if let Some(target) = target {
        if target.account_id.trim().is_empty() || target.subscription_id.trim().is_empty() {
            return Err("billingTarget accountId and subscriptionId are required".into());
        }
        if !provider_matches(&target.provider_id, provider) {
            return Err(format!(
                "billingTarget provider '{}' does not match model provider '{provider}'",
                target.provider_id
            ));
        }
    }
    Ok(entries
        .into_iter()
        .filter(|entry| {
            provider_matches(&entry.provider, provider)
                && entry.status == "active"
                && !crate::journal::is_retired(&entry.id)
                && target.is_none_or(|target| entry.id == target.subscription_id)
        })
        .collect())
}
