//! A grant the provider already disowned is not presented to it again.
//!
//! The usage ledger keeps that verdict, with the provider's own sentence,
//! until a sign-in stores a new grant, and only that sign-in clears it. Asking
//! the provider to refresh the same grant once more repeats a refusal it
//! already gave, on every plan reading sweep and every rejected request, and
//! delays the answer that a sign-in is the repair. An operator who doubts the
//! verdict retries it on purpose through `brama subscription refresh`, which
//! is the one caller that passes this gate.

use tracing::warn;
use wisent_errors::{Code, Failure};

use crate::core::failure::{self, IMPACT_CREDENTIAL_PERSIST, POINT_CREDENTIAL_PERSIST};
use crate::gateway::oauth_refresh;

/// The refusal for a subscription whose grant the ledger holds awaiting a
/// sign-in, or `None` when its grant may be refreshed.
pub(super) fn refusal(subscription_id: &str, provider: &str) -> Option<Failure> {
    let cause = crate::subscription_dispatch::usage::awaiting_sign_in_cause(subscription_id)?;
    Some(
        failure::envelope(
            POINT_CREDENTIAL_PERSIST,
            Code::Auth,
            IMPACT_CREDENTIAL_PERSIST,
            format!(
                "subscription `{subscription_id}` is awaiting a sign-in ({cause}); its grant is not refreshed again until a sign-in stores a new one"
            ),
        )
        .with_context("subscription", subscription_id)
        .with_context("provider", provider),
    )
}

/// Put a refused refresh in the ledger when the provider disowned the grant,
/// and leave the record alone when nothing about the grant was learned.
///
/// Both refresh paths classify here -- the sweep ahead of expiry and the forced
/// refresh a rejected request triggers -- so a credential cannot read as dead on
/// one path and healthy on the other. The disowned verdict this records is
/// what [`refusal`] reads.
pub(super) fn record_refusal(subscription_id: &str, provider: &str, refused: &Failure) {
    // The provider's own sentence, or the statement that it gave none: the
    // ledger's cause is what an operator reads, and an empty cause is a row
    // that says a sign-in is needed without saying why.
    let detail = match refused.detail.as_deref() {
        Some(detail) => detail,
        None => "the provider refused this credential's refresh without saying why",
    };
    match oauth_refresh::classify_refusal(refused) {
        oauth_refresh::RefreshRefusal::Definitive => {
            warn!(
                event = "credential_refresh_refused_definitively",
                subscription = %subscription_id,
                provider,
                error = detail,
                "the provider will not accept this grant again; only a sign-in repairs it"
            );
            crate::subscription_dispatch::usage::record_reauthorization_needed(
                subscription_id,
                provider,
                detail,
            );
        }
        oauth_refresh::RefreshRefusal::Transient => {
            warn!(
                event = "credential_refresh_transient_skipped",
                subscription = %subscription_id,
                provider,
                error = detail,
                "the refresh failed without the provider disowning the grant; the \
                 credential is left as it stands for the next sweep"
            );
        }
    }
}
