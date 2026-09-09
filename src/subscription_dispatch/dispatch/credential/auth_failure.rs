//! Reading a provider's refusal for what it says about the credential: a
//! token that is merely stale, or a grant that is gone.

use crate::subscription_dispatch::usage;

pub(in crate::subscription_dispatch::dispatch) fn is_permanent_auth_failure(error: &str) -> bool {
    let error = error.to_ascii_lowercase();
    error.contains("refresh token was revoked")
        || error.contains("access token could not be refreshed")
        || error.contains("invalid_grant")
}

pub(in crate::subscription_dispatch::dispatch) fn is_auth_failure(error: &str) -> bool {
    let error = error.to_ascii_lowercase();
    error.contains("invalid authentication")
        || error.contains("authentication_error")
        || error.contains("failed to authenticate")
        || error.contains("401")
        || error.contains("provider_authentication")
        || error.contains("oauth")
        || is_permanent_auth_failure(&error)
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
