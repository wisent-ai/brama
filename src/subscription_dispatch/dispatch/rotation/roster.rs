//! Which of an agent's accounts this one call may be billed to, in the order
//! the ledger and the pin put them.

use crate::core::failure::POINT_CREDENTIAL_SELECTION;
use crate::gateway::broker;
use crate::subscription_dispatch::usage;
use crate::types::{GatewayRefusal, ModelRequest, ModelResponse};

use super::super::credential::eligibility::eligible_subscription_entries;
use super::super::ranking::pin::apply_pin;
use super::super::refusal::envelope::refuse;
use super::super::refusal::pool_empty::no_active_credential_summary;

pub(super) fn max_credential_attempts() -> usize {
    2
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
    let entries = broker::routing_subscriptions(agent_id).map_err(|error| {
        refuse(
            request,
            POINT_CREDENTIAL_SELECTION,
            GatewayRefusal::DependencyUnavailable,
            format!(
                "route {}: no live subscription; discovery failed: {error}",
                request.model
            ),
            None,
        )
    })?;
    let mut rows =
        eligible_subscription_entries(entries, provider, request.billing_target.as_ref()).map_err(
            |error| {
                // The billing target the caller named does not fit this route.
                ModelResponse::refused(&request.model, GatewayRefusal::InvalidRequest, error)
            },
        )?;
    if rows.is_empty() {
        // Not capacity. This agent holds no account this call could be billed
        // to at all, or the one it named is inactive, and no wait repairs
        // either: the sign-in or the vault grant has to be repaired. Answered
        // as capacity, it read as "try again" to every client - Jeden retried
        // twice and then reported a stream timeout, while Brama had known from
        // the first attempt that nothing could pay for the call. `verdict.rs`
        // records this same defect twice, one layer further out each time;
        // this is the layer where the pool is empty before any provider is
        // asked.
        return Err(refuse(
            request,
            POINT_CREDENTIAL_SELECTION,
            GatewayRefusal::CredentialUnauthorized,
            request.billing_target.as_ref().map_or_else(
                || format!("route {}: no live subscription; {}", request.model, no_active_credential_summary(provider)),
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
    // A credential inside a recorded block is walked last. The walk takes the
    // first `max_credential_attempts` rows and skips the blocked ones among
    // them without asking a provider, so a blocked row at the front spends
    // an attempt a live row could have used. A burnt subscription has no
    // plan reading, and no reading sorts as the freest plan: with live
    // accounts behind burnt ones, every `best` call takes the burnt rows,
    // walks nothing, and is refused `all bounded credentials were rejected
    // by the provider; re-authorization required` with `attempts: 0` while
    // the live accounts are never tried. The blocked rows stay in the list, after
    // the live ones, so a pool with nothing but blocks still reports the
    // block it is inside.
    let (live, blocked): (Vec<_>, Vec<_>) = rows
        .into_iter()
        .partition(|row| !usage::is_blocked(&row.id));
    let mut rows = live;
    rows.extend(blocked);
    Ok(rows)
}
