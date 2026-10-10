//! Saved provider resets are capacity owned already, before another account purchase.

pub mod declaration;
pub mod maintenance;
pub mod model;
mod policy;
pub mod redeem;

use crate::gateway::broker;
use crate::providers::adapter;
use crate::subscription_dispatch::usage;
use model::ResetOffer;

pub async fn refresh(member: &str, provider: &str) -> Result<ResetOffer, String> {
    let declared = super::declaration::provider(provider)?
        .resets
        .as_ref()
        .ok_or_else(|| format!("provider {provider} declares no saved-reset report"))?;
    if crate::journal::is_retired(member) {
        return Err(format!(
            "subscription {member} is retired; its grant is not used"
        ));
    }
    if usage::usage_for(member)
        .and_then(|entry| entry.credential)
        .and_then(|credential| credential.borrowed_from)
        .is_some()
    {
        return Err(format!("subscription {member} carries a harness-owned grant; reset operations require Brama's own sign-in"));
    }
    let outcome = async {
        let credential = broker::subscription_credential(member, provider)
            .await
            .map_err(|error| error.to_json())?;
        let secret = credential
            .expose_utf8()
            .map_err(|error| format!("subscription credential: {error}"))?;
        let item = broker::subscription_resource(provider, member);
        adapter::read_reset_offers(provider, &item, secret, declared).await
    }
    .await;
    usage::record_reset_offer(member, provider, outcome.clone())?;
    outcome
}
