//! Which of an agent's accounts this one call may be billed to, in the order
//! the ledger and the pin put them.

use crate::core::failure::POINT_CREDENTIAL_SELECTION;
use crate::gateway::broker;
use crate::subscription_dispatch::usage;
use crate::types::{ModelRequest, ModelResponse};

use super::super::credential::eligibility::eligible_subscription_entries;
use super::super::ranking::pin::apply_pin;
use super::super::refusal::envelope::refuse;
use super::super::refusal::pool_empty::no_active_credential_summary;

pub(super) fn max_credential_attempts() -> usize {
    "2".parse().expect("valid credential attempt limit")
}

/// The accounts one route may rotate across, already ordered.
///
/// The buffered and streaming paths reach this list identically, and the
/// refusal a caller reads when the list is empty is the same sentence either
/// way; writing it twice is how two callers of one broken deployment come to
/// read two different faults.
///
/// An `Err` here is always a refusal that emptied the pool before any provider
/// was asked: nothing this agent holds could have paid for the call.
pub(super) async fn ordered_candidate_rows(
    provider: &str,
    agent_id: &str,
    request: &ModelRequest,
) -> Result<Vec<broker::SubscriptionEntry>, ModelResponse> {
    let mut rows = eligible_subscription_entries(
        broker::list_subscriptions(agent_id).await,
        provider,
        request.billing_target.as_ref(),
    )
    .map_err(|error| ModelResponse::failure(&request.model, error))?;
    if rows.is_empty() {
        return Err(refuse(
            request,
            POINT_CREDENTIAL_SELECTION,
            request.billing_target.as_ref().map_or_else(
                || no_active_credential_summary(provider),
                |target| {
                    format!(
                        "selected credential '{}' is not active for provider '{provider}' and agent",
                        target.subscription_id
                    )
                },
            ),
            None,
        ));
    }

    // Freest plan first, the ledger's own numbers rather than list order; the
    // agent's pinned credential then leads unless its window says it is full.
    // Both are reorderings of the same bounded list -- the attempt cap is
    // untouched, and an explicit billing target has one row to reorder.
    rows.sort_by(|left, right| {
        usage::used_fraction(&left.id)
            .unwrap_or(0.0)
            .partial_cmp(&usage::used_fraction(&right.id).unwrap_or(0.0))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    apply_pin(&mut rows, agent_id, provider);
    Ok(rows)
}
