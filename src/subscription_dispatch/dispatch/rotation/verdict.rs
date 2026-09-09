//! The one refusal an emptied bounded pool is answered with, whichever path
//! emptied it.

use crate::core::failure::POINT_BOUNDED_ROTATION;
use crate::types::{ModelRequest, ModelResponse};
use wisent_errors::Failure;

use super::super::refusal::envelope::{failure_detail, refuse, refuse_as};
use super::super::refusal::pool_empty::{
    bounded_unavailable_summary, pool_empty_summary, pool_is_capacity, rotation_failure_kind,
    PoolEmptyCause,
};

/// What one walk of a provider's bounded pool actually saw, as opposed to what
/// it kept: the four ways the pool can empty, each recorded where it happened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct PoolObservations {
    /// A provider refused a credential outright during this request.
    pub(super) auth_rejection: bool,
    /// A credential was skipped because its recorded block is an authorization
    /// block rather than a rate limit.
    pub(super) reauthorization_block: bool,
    /// The vault produced no credential at all -- no capability, no grant.
    pub(super) unredeemable_credential: bool,
    /// A credential was skipped because its recorded block is a rate limit.
    pub(super) rate_limit_block: bool,
}

/// Refuse the request the way an emptied pool is refused, with the attempt
/// count the walk actually paid for.
pub(super) fn emptied_pool_refusal(
    request: &ModelRequest,
    provider: &str,
    provider_attempts: u32,
    credential_refusal: Option<Failure>,
    observed: PoolObservations,
    rate_limit_failure: Option<ModelResponse>,
) -> ModelResponse {
    // A usable account's quota reset can serve the request even when another
    // account needs authorization. Preserve its actual provider refusal.
    if let Some(mut failure) = rate_limit_failure {
        failure.attempts = provider_attempts;
        return failure;
    }
    let cause = PoolEmptyCause {
        auth_rejection: observed.auth_rejection,
        reauthorization_block: observed.reauthorization_block,
        unredeemable_credential: observed.unredeemable_credential,
    };
    // Capacity only when nothing authorization-shaped was seen; the rule lives
    // in `pool_empty::pool_is_capacity`, beside the sentences it chooses
    // between, because this arm used to come first and answered capacity for a
    // pool whose every credential the ledger said needed a sign-in.
    if pool_is_capacity(cause, observed.rate_limit_block) {
        let mut failure = refuse(
            request,
            POINT_BOUNDED_ROTATION,
            bounded_unavailable_summary(provider),
            None,
        );
        failure.attempts = provider_attempts;
        return failure;
    }
    let message = credential_refusal
        .as_ref()
        .map(|refused| {
            format!(
                "'{provider}' subscription credential failed: {}",
                failure_detail(refused)
            )
        })
        .unwrap_or_else(|| pool_empty_summary(provider, cause));
    let mut failure = refuse_as(
        request,
        POINT_BOUNDED_ROTATION,
        rotation_failure_kind(cause),
        message,
        credential_refusal,
    );
    failure.attempts = provider_attempts;
    failure
}
