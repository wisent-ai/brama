//! The candidate lists a selector walks: every route this agent can be served
//! from, in the order the ledger ranks them.

use super::plan_order::order_models_by_plan;
use super::super::catalogue::subscription_models::registry_models_for_agent;

/// The candidate list `best` walks: every subscription model this agent can be
/// served from, freest plan first, with the alias's configured route promoted
/// to the head when the agent actually holds it.
///
/// The configured route is a preference, not the whole list. An operator
/// pointing `best` at `codex/gpt-5.3-codex-spark` is naming the model they want
/// first, not consenting to a fleet outage every time one provider's credential
/// chain breaks.
pub(in crate::subscription_dispatch::dispatch) async fn best_subscription_models(
    agent_id: &str,
    preferred: Option<&str>,
) -> Result<Vec<String>, String> {
    let mut models = active_supported_models_for_agent(agent_id).await?;
    if let Some(position) =
        preferred.and_then(|preferred| models.iter().position(|model| model == preferred))
    {
        // Rotate rather than swap: everything behind the preferred route keeps
        // the plan order the ledger just computed for it.
        models[..=position].rotate_right(1);
    }
    Ok(models)
}

pub async fn active_supported_models_for_agent(agent_id: &str) -> Result<Vec<String>, String> {
    let mut models = registry_models_for_agent(agent_id)
        .await?
        .into_iter()
        .map(|model| model.route_id)
        .collect::<Vec<_>>();
    if models.is_empty() {
        return Err("no active stateless provider models for signed agent".into());
    }
    order_models_by_plan(agent_id, &mut models).await?;
    Ok(models)
}

pub async fn active_vision_capable_models_for_agent(agent_id: &str) -> Result<Vec<String>, String> {
    let mut models = registry_models_for_agent(agent_id)
        .await?
        .into_iter()
        .filter(|model| model.input_modalities.iter().any(|value| value == "image"))
        .map(|model| model.route_id)
        .collect::<Vec<_>>();
    if models.is_empty() {
        return Err("no active vision-capable stateless provider model for signed agent".into());
    }
    order_models_by_plan(agent_id, &mut models).await?;
    Ok(models)
}
