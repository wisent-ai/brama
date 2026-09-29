//! The one refusal an emptied bounded pool is answered with, whichever path
//! emptied it.

use crate::core::failure::POINT_BOUNDED_ROTATION;
use crate::types::{GatewayRefusal, ModelRequest, ModelResponse, ProviderRefusal};
use wisent_errors::Failure;

use super::super::refusal::envelope::{
    credential_refusal_class, failure_detail, refuse, refuse_as,
};
use super::super::refusal::pool_empty::{
    capacity_is_mixed, capacity_summary, pool_empty_summary, pool_is_capacity, PoolEmptyCause,
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
    /// A credential was skipped because its recorded block is a spent paid
    /// balance, which no wait lifts.
    pub(super) quota_exhausted_block: bool,
    /// When the soonest of those blocks lifts, when the ledger recorded it.
    pub(super) block_lifts_at_ms: Option<i64>,
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
    // Capacity when one member is blocked by quota alone: the walk reads the
    // ledger per credential, so such a block means a member that is otherwise
    // usable and a wait that reaches it. The sentence says whether other
    // members also need a sign-in.
    if pool_is_capacity(observed.rate_limit_block) {
        let summary = capacity_summary(
            provider,
            capacity_is_mixed(cause),
            observed.block_lifts_at_ms,
        );
        let mut failure = refuse(
            request,
            POINT_BOUNDED_ROTATION,
            GatewayRefusal::SubscriptionUnavailable,
            summary,
            None,
        );
        failure.attempts = provider_attempts;
        return failure;
    }
    // Every member the walk met is out of paid balance and none needs a
    // sign-in: that is the answer the first request gave from the provider's
    // own refusal, and the ledger now says it again instead of calling it a
    // wait.
    if observed.quota_exhausted_block
        && credential_refusal.is_none()
        && !cause.needs_authorization()
    {
        let mut failure = refuse_as(
            request,
            POINT_BOUNDED_ROTATION,
            ProviderRefusal::QuotaExhausted,
            format!("every usable '{provider}' credential has spent its paid balance"),
            None,
        );
        failure.attempts = provider_attempts;
        return failure;
    }
    // A lower layer's refusal is carried in its own words, with the class its
    // failure code states; every other emptied pool states its class from why
    // it emptied.
    let mut failure = match credential_refusal {
        Some(refused) => refuse(
            request,
            POINT_BOUNDED_ROTATION,
            credential_refusal_class(&refused),
            format!(
                "'{provider}' subscription credential failed: {}",
                failure_detail(&refused)
            ),
            Some(refused),
        ),
        None => refuse(
            request,
            POINT_BOUNDED_ROTATION,
            pool_empty_class(cause),
            pool_empty_summary(provider, cause),
            None,
        ),
    };
    failure.attempts = provider_attempts;
    failure
}

/// The class an emptied pool is answered with, from the same cause its
/// sentence is chosen from.
///
/// A provider that refused every credential and a vault that produced none are
/// both authorization failures no wait repairs; only a genuinely exhausted pool
/// is capacity. A credential inside an authorization block counts with the
/// first group: the router skips it without calling the provider, so for the
/// half hour its block lasts it looks exactly like one out of quota, and
/// answering that as capacity told the caller to retry.
fn pool_empty_class(cause: PoolEmptyCause) -> GatewayRefusal {
    if cause.auth_rejection || cause.reauthorization_block {
        GatewayRefusal::SubscriptionReauthorizationRequired
    } else if cause.unredeemable_credential {
        GatewayRefusal::CredentialUnauthorized
    } else {
        GatewayRefusal::SubscriptionUnavailable
    }
}
