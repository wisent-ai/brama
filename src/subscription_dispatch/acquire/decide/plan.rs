//! Which plan a new account is bought on: the plan the pool's own accounts
//! hold, as the provider states it for each of them.
//!
//! No plan is chosen here. Every member whose grant answers the provider's
//! profile names its tier; when they all name the same one, that is the plan
//! the operator pays for and the new account gets it. Members that disagree,
//! or a pool where no grant answers, stop the acquisition with the tiers and
//! refusals that were read, because buying a plan nobody holds would be a
//! choice made in code.

use std::collections::BTreeMap;

use crate::gateway::broker;
use crate::providers::adapter as provider_registry;

use super::pool_state::Member;

/// The tier every answering member holds, or the reason none can be named.
pub(super) async fn pool_tier(provider: &str, members: &[Member]) -> Result<String, String> {
    let mut tiers: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut refusals = Vec::new();
    for member in members {
        match member_tier(provider, &member.id).await {
            Ok(tier) => tiers.entry(tier).or_default().push(member.id.clone()),
            Err(detail) => refusals.push(format!("{}: {detail}", member.id)),
        }
    }
    let mut named = tiers.into_iter();
    match (named.next(), named.next()) {
        (Some((tier, _)), None) => Ok(tier),
        (None, _) => Err(format!(
            "no {provider} member's grant states its plan tier, so the plan to buy cannot be \
             read: {}",
            refusals.join("; ")
        )),
        (Some(first), Some(second)) => {
            let disagreeing = std::iter::once(first)
                .chain(std::iter::once(second))
                .chain(named)
                .map(|(tier, ids)| format!("{tier} ({})", ids.join(", ")))
                .collect::<Vec<_>>()
                .join("; ");
            Err(format!(
                "the {provider} pool holds more than one plan tier, so which one to buy is not \
                 stated by the pool: {disagreeing}"
            ))
        }
    }
}

async fn member_tier(provider: &str, subscription_id: &str) -> Result<String, String> {
    let item = broker::subscription_resource(provider, subscription_id);
    let credential = broker::subscription_credential(subscription_id, provider)
        .await
        .map_err(|failure| format!("its grant could not be read: {failure}"))?;
    let token = credential
        .expose_utf8()
        .map_err(|_| format!("the grant in `{item}` is not UTF-8 text"))?;
    provider_registry::read_plan_tier(provider, &item, token).await
}
