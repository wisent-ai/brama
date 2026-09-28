//! Reading a provider's refusal for what it says about the credential, from
//! the class the adapter took off the answer's status. The provider's sentence
//! is data and never decides: a request error that happens to mention OAuth or
//! a limit is not an account problem, and a reworded quota answer still is one.
//!
//! Whether a refused credential is merely stale or gone for good is not read
//! here either: the forced refresh answers that. A refresh the provider refuses
//! definitively is recorded as needing a sign-in by the broker's renewal, and a
//! token refused right after the provider issued it is retired by the rotation.

use crate::subscription_dispatch::usage;
use crate::types::{ModelResponse, ProviderRefusal};

/// The provider refused the credential itself (401 or 403).
pub(in crate::subscription_dispatch::dispatch) fn refused_credential(
    response: &ModelResponse,
) -> bool {
    response.failure_kind == Some(ProviderRefusal::Authentication)
}

/// The provider refused because the credential's rate window or paid balance
/// is spent; another account may serve.
pub(in crate::subscription_dispatch::dispatch) fn exhausted_credential(
    response: &ModelResponse,
) -> bool {
    matches!(
        response.failure_kind,
        Some(ProviderRefusal::RateLimited | ProviderRefusal::QuotaExhausted)
    )
}

/// Retire one credential the provider has permanently refused, and say so in the
/// ledger.
///
/// The journal is what stops this credential being selected again; the ledger
/// record is what lets a reader see that it was retired, when, and why. Without
/// the second one a retired subscription and a subscription nobody has used all
/// month render as the same row.
pub(in crate::subscription_dispatch::dispatch) async fn mark_credential_revoked(
    credential_id: &str,
    provider: &str,
    cause: &str,
) {
    crate::journal::retire(credential_id);
    usage::record_credential_disabled(credential_id, provider, cause);
}
