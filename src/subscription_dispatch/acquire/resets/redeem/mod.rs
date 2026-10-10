//! Serialize a member's reset and retain an ambiguous outcome across process restarts.

use super::{
    model::{ResetRedemption, ResetState},
    policy,
};
use crate::gateway::broker;
use crate::journal::resets as journal;
use crate::subscription_dispatch::usage;
use serde_json::{json, Value};

pub async fn run(
    provider: &str,
    member: &str,
    reason: &str,
    automatic: bool,
) -> Result<Value, String> {
    if reason.trim().is_empty() {
        return Err("--reason must state why this subscription reset is requested".into());
    }
    let _lock = journal::lock(member)?;
    let members = broker::list_all_subscriptions().await?;
    let selected = members
        .iter()
        .find(|entry| entry.id == member)
        .ok_or_else(|| format!("subscription {member} does not exist in the serving vault"))?;
    if selected.provider != provider {
        return Err(format!(
            "subscription {member} belongs to {}, not {provider}",
            selected.provider
        ));
    }
    let declared = super::super::declaration::provider(provider)?
        .resets
        .as_ref()
        .ok_or_else(|| format!("provider {provider} declares no reset operation"))?;
    let offer = super::refresh(member, provider).await?;
    if let Some(mut previous) = journal::latest(member)? {
        if previous.state.unresolved() {
            let consumed = offer
                .credits
                .iter()
                .find(|credit| credit.id == previous.credit_id)
                .is_some_and(|credit| {
                    credit.consumption_observable
                        && credit.remaining_count < previous.remaining_before
                });
            if !consumed {
                return Err(format!("subscription {member}: reset request {} remains {:?}; the provider report has not confirmed consumption, so no other reset is sent", previous.request_id, previous.state));
            }
            previous.state = ResetState::ConsumptionObserved;
            previous.detail = Some("a fresh provider report shows fewer uses of the selected credit; quota restoration is not inferred".into());
            previous.at_ms = chrono::Utc::now().timestamp_millis();
            persist(&previous)?;
            return Ok(
                json!({"ok": true, "result": "reconciled", "redemption": previous, "offer": offer}),
            );
        }
    }
    let blocked_until = policy::blocked_until(member, provider).await?;
    let Some((credit, trigger)) = policy::choose(&offer, declared, blocked_until, automatic)?
    else {
        return Ok(
            json!({"ok": automatic, "result": "refused", "provider": provider, "member": member,
            "detail": "no usable reset meets this member's provider eligibility and reset policy", "offer": offer}),
        );
    };
    let credential = broker::subscription_credential(member, provider)
        .await
        .map_err(|error| error.to_json())?;
    let secret = credential
        .expose_utf8()
        .map_err(|error| format!("subscription credential: {error}"))?;
    let mut outcome = ResetRedemption {
        provider: provider.to_owned(),
        member: member.to_owned(),
        credit_id: credit.id.clone(),
        remaining_before: credit.remaining_count,
        request_id: uuid::Uuid::new_v4().to_string(),
        reason: reason.to_owned(),
        state: ResetState::Requested,
        detail: Some(trigger.into()),
        at_ms: chrono::Utc::now().timestamp_millis(),
    };
    let item = broker::subscription_resource(provider, member);
    let prepared = crate::providers::adapter::prepare_reset_redemption(
        provider,
        &item,
        secret,
        declared,
        credit,
        &outcome.request_id,
    )
    .await;
    let prepared = match prepared {
        Ok(prepared) => prepared,
        Err(error) => {
            outcome.state = ResetState::Refused;
            outcome.detail = Some(format!("reset was not sent: {error}"));
            outcome.at_ms = chrono::Utc::now().timestamp_millis();
            persist(&outcome)?;
            return Err(format!(
                "subscription {member}: reset was not sent: {error}"
            ));
        }
    };
    persist(&outcome)?;
    let response = stado_wait::until(
        stado_wait::Kind::Network,
        "send journaled subscription reset",
        member,
        prepared.send(),
    )
    .await;
    match response {
        Ok(response) => {
            let confirmed = response["ok"].as_bool() == Some(true);
            outcome.state = if confirmed {
                ResetState::Redeemed
            } else if response["not_applied"].as_bool() == Some(true) {
                ResetState::Refused
            } else {
                ResetState::Unconfirmed
            };
            outcome.detail = Some(response.to_string());
            outcome.at_ms = chrono::Utc::now().timestamp_millis();
            persist(&outcome)?;
            let fresh_offer = super::refresh(member, provider).await;
            let quota = policy::blocked_until(member, provider).await;
            if confirmed && matches!(quota, Ok(None)) {
                usage::record_reset_unblocked(member, provider)?;
            }
            let refreshed = fresh_offer.is_ok() && quota.is_ok();
            Ok(
                json!({"ok": confirmed && refreshed, "result": outcome.state, "redemption": outcome,
                "offer": fresh_offer.as_ref().ok(), "offer_error": fresh_offer.as_ref().err(),
                "blocked_until_ms": quota.as_ref().ok(), "quota_error": quota.as_ref().err()}),
            )
        }
        Err(error) => {
            outcome.state = ResetState::Unconfirmed;
            outcome.detail = Some(error.clone());
            outcome.at_ms = chrono::Utc::now().timestamp_millis();
            persist(&outcome)?;
            Err(format!("subscription {member}: reset {} is unconfirmed: {error}; no mutation retry was sent", outcome.request_id))
        }
    }
}

fn persist(outcome: &ResetRedemption) -> Result<(), String> {
    journal::record(outcome)?;
    usage::record_reset_redemption(outcome.clone())
}
