//! One subscription's turn: replacing a single grant, and saying what came of
//! it.
//!
//! The sweep counts outcomes; this produces them. Each attempt runs in a task
//! of its own so that a panic underneath -- in a credential parser, a provider
//! client or the vault writer -- ends one subscription's turn rather than the
//! timer that drives every other subscription. And each answer the broker gives
//! is reduced here to the one fact the sweep can count: the grant was not due,
//! it was replaced, it was refused, or the vault row held no credential at all
//! and only a sign-in can repair it.
//!
//! Keeping this apart from the walk is what makes the walk readable as a walk:
//! selecting subscriptions and surviving one of them are different problems,
//! and only this one has to know what the broker's four verdicts mean.

use std::time::Duration;

use tracing::{info, warn};

use crate::gateway::broker::{self, RefreshAhead};
use crate::subscription_dispatch::usage;

use super::claim::InFlight;

/// What one subscription's turn came to, so the sweep can say what it did
/// rather than only that it ran.
pub(super) enum Swept {
    /// This grant has more than the skew window left.
    NotDue,
    /// The access token was replaced before it expired.
    Refreshed,
    /// The refresh was refused, or the refreshed grant could not be stored.
    Refused,
    /// The vault row exists but yielded no credential. Its exact account must
    /// go through Weles rather than being left unknown forever.
    AwaitingSignIn {
        subscription_id: String,
        provider: String,
    },
    /// A refresh for this subscription was already running.
    Skipped,
}

/// Refresh one subscription, surviving anything it does.
///
/// The work runs in its own task so that a panic below -- in a credential
/// parser, a provider client or the vault writer -- is a logged join error
/// against one subscription instead of the silent death of the whole timer.
pub(super) async fn refresh_one(
    subscription_id: String,
    provider: String,
    skew: Duration,
) -> Swept {
    let logged_id = subscription_id.clone();
    let logged_provider = provider.clone();
    match tokio::spawn(refresh_subscription(subscription_id, provider, skew)).await {
        Ok(swept) => swept,
        Err(error) => {
            warn!(
                event = "credential_refresh_panicked",
                subscription = %logged_id,
                provider = %logged_provider,
                %error,
                "a credential refresh died; the remaining subscriptions are unaffected"
            );
            Swept::Skipped
        }
    }
}

/// Refresh one subscription's grant when it expires inside the skew window.
async fn refresh_subscription(subscription_id: String, provider: String, skew: Duration) -> Swept {
    let Some(_claim) = InFlight::claim(&subscription_id) else {
        info!(
            event = "credential_refresh_already_running",
            subscription = %subscription_id,
            provider = %provider,
            "a refresh for this subscription is still running; this sweep leaves it alone"
        );
        return Swept::Skipped;
    };
    match broker::refresh_subscription_credential_ahead(&subscription_id, &provider, skew).await {
        RefreshAhead::NotDue { expires_at_ms } => {
            // Recorded so a reader can say until when this grant is good, which
            // is the question the console could not answer at all. The ledger
            // ignores a record that changes nothing, so this does not make every
            // row look freshly observed once a minute.
            usage::record_credential_active(&subscription_id, &provider, expires_at_ms, false);
            Swept::NotDue
        }
        RefreshAhead::Refreshed { expires_at_ms } => {
            info!(
                event = "credential_refreshed_ahead",
                subscription = %subscription_id,
                provider = %provider,
                expires_at_ms,
                skew_secs = skew.as_secs(),
                "replaced an access token before it expired"
            );
            Swept::Refreshed
        }
        // Both refusals are already classified, logged and recorded where the
        // refresh happened, so that the forced refresh a rejected request
        // triggers reaches the same verdict as this sweep does.
        RefreshAhead::Refused(_) => Swept::Refused,
        RefreshAhead::Unavailable(refused) => {
            warn!(
                event = "credential_refresh_unavailable",
                subscription = %subscription_id,
                provider = %provider,
                error = refused.detail.as_deref().unwrap_or_default(),
                envelope = %refused.to_json(),
                "no credential could be read for this subscription; its declared Weles account \
                 will be asked to restore the grant"
            );
            Swept::AwaitingSignIn {
                subscription_id,
                provider,
            }
        }
    }
}
