//! Replacing one subscription's OAuth grant, and recording what came of it.
//!
//! Reading a stored credential is the caller's business next door; this is
//! what happens when the stored one is about to die or the provider has
//! already stopped accepting it. Three things make it its own subject: the
//! rotation is single-flight, so the credential is deliberately re-read under
//! the lock; a rotated grant that cannot be stored is a failed refresh rather
//! than a successful one, because the provider has already invalidated what
//! the vault still holds; and a refusal has to be classified once, so a
//! credential cannot read as dead on the sweep's path and healthy on the
//! rejected request's.

use std::sync::LazyLock;

use tracing::warn;

use super::super::redeem_subscription_credential;
use crate::capability::Secret;
use crate::core::failure::{self, IMPACT_CREDENTIAL_PERSIST, POINT_CREDENTIAL_PERSIST};
use crate::gateway::oauth_refresh;
use wisent_errors::{Code, Failure};

static OAUTH_REFRESH_LOCK: LazyLock<tokio::sync::Mutex<()>> =
    LazyLock::new(|| tokio::sync::Mutex::new(()));

pub(in crate::gateway::broker) async fn refresh_subscription_credential_inner(
    subscription_id: &str,
    provider: &str,
    force: bool,
) -> Result<Secret, Failure> {
    if let Some(refused) = awaiting::refusal(subscription_id, provider) {
        return Err(refused);
    }
    refresh_presented(subscription_id, provider, force).await
}

async fn refresh_presented(
    subscription_id: &str,
    provider: &str,
    force: bool,
) -> Result<Secret, Failure> {
    // Refresh-token rotation is single-flight. Re-reading after the lock lets a
    // concurrent caller observe the value already written to the vault.
    let _guard = OAUTH_REFRESH_LOCK.lock().await;
    let credential = redeem_subscription_credential(subscription_id, provider).await?;
    // Reject legacy harness copies before either use or refresh, even when
    // their access token has not expired. Reconciliation can retire the member;
    // only independent Weles sign-in supplies this gateway's own grant.
    if is_borrowed(&credential) {
        let detail = format!(
            "subscription `{subscription_id}` carries a borrowed harness grant; Brama neither uses nor rotates it. Obtain an independent grant through Weles; `brama subscription list` retains the sign-in refusal when that cannot complete"
        );
        crate::subscription_dispatch::usage::record_reauthorization_needed(
            subscription_id,
            provider,
            &detail,
        );
        let refused = failure::envelope(
            POINT_CREDENTIAL_PERSIST,
            Code::Config,
            IMPACT_CREDENTIAL_PERSIST,
            detail,
        )
        .with_context("subscription", subscription_id)
        .with_context("provider", provider);
        return Err(refused);
    }
    if !force && !oauth_refresh::needs_refresh(&credential, provider) {
        return Ok(credential);
    }
    let mut fresh = match oauth_refresh::refresh(&credential, provider).await {
        Ok(fresh) => fresh,
        Err(refused) => {
            let refused = refused
                .with_context("subscription", subscription_id)
                .with_context("provider", provider);
            warn!(
                event = "oauth_refresh_failed",
                provider,
                error = refused.detail.as_deref(),
                envelope = %refused.to_json(),
                "OAuth refresh failed"
            );
            awaiting::record_refusal(subscription_id, provider, &refused);
            return Err(refused);
        }
    };
    // The reason matters more than the fact. A refreshed grant that cannot be
    // written is used once and lost, so the stale one returns on the next
    // start and the subscription reads as dead -- while this line said only
    // that something went wrong. The default recipient being a key no keyring
    // holds looked identical to a broken vault for a full day.
    if let Err(error) = put_subscription_credential(subscription_id, provider, &fresh).await {
        let persist_detail = error.clone();
        let unpersisted = failure::envelope(
            POINT_CREDENTIAL_PERSIST,
            Code::Config,
            IMPACT_CREDENTIAL_PERSIST,
            error,
        )
        .with_context("subscription", subscription_id)
        .with_context("provider", provider);
        warn!(
            event = "oauth_refresh_persist_failed",
            provider,
            error = %persist_detail,
            envelope = %unpersisted.to_json(),
            "refreshed OAuth credential could not be persisted; the rotated grant is lost \
             and the stored one is already dead at the provider"
        );
        // Using it once and moving on is what turned working accounts into
        // permanent `invalid_grant`: the provider rotated the refresh token, the
        // new one was never written, and the vault kept a grant the provider had
        // already invalidated. So this is a failed refresh rather than a
        // successful one with a warning attached: the grant is dropped here
        // instead of being spent from memory, and the subscription is recorded
        // as needing a re-authorization so the renewal path runs.
        crate::subscription_dispatch::usage::record_reauthorization_needed(
            subscription_id,
            provider,
            &persist_detail,
        );
        return Err(unpersisted);
    }
    let refreshed = Secret::from_bytes(std::mem::take(&mut *fresh));
    // What the vault now holds, said in the ledger rather than left to be
    // rediscovered: a reader asking why a token died early needs the instant it
    // was rotated and the instant it expires, and neither is in the vault.
    let expires_at_ms = oauth_refresh::access_token_expiry_ms(&refreshed, provider);
    crate::subscription_dispatch::usage::record_credential_active(
        subscription_id,
        provider,
        expires_at_ms,
        true,
    );
    Ok(refreshed)
}

mod awaiting;
mod storing;

pub(in crate::gateway::broker) use storing::is_borrowed;
use storing::unusable_document;
pub use storing::{
    put_subscription_credential, put_subscription_credential_for_account, subscription_account,
};

/// Force one OAuth refresh after the provider rejects a grant whose local
/// expiry still claims it is valid. The rejected grant is not returned when
/// refresh fails because retrying it would only repeat the provider error; the
/// refusal itself is, so the caller can report what the provider said instead
/// of reporting that something happened.
pub async fn refresh_subscription_credential(
    subscription_id: &str,
    provider: &str,
) -> Result<Secret, Failure> {
    refresh_subscription_credential_inner(subscription_id, provider, true).await
}

/// The operator's deliberate retry of a grant the ledger holds awaiting a
/// sign-in: the one refresh that presents a disowned grant again, because an
/// operator who doubts the provider's verdict asked for it by name.
pub async fn retry_disowned_subscription_credential(
    subscription_id: &str,
    provider: &str,
) -> Result<Secret, Failure> {
    refresh_presented(subscription_id, provider, true).await
}

/// Whether this provider's subscription credentials are OAuth grants that can be
/// refreshed, rather than API keys that never expire.
pub fn supports_oauth_refresh(provider: &str) -> bool {
    oauth_refresh::supports_refresh(provider)
}

/// What one refresh-ahead attempt concluded, for a caller that never holds the
/// credential itself.
pub enum RefreshAhead {
    /// This grant has not expired, or the provider is not one
    /// whose credentials Brama refreshes at all. `expires_at_ms` is present only
    /// when the credential states an expiry.
    NotDue { expires_at_ms: Option<i64> },
    /// The grant was refreshed and the new one is in the vault.
    Refreshed { expires_at_ms: Option<i64> },
    /// The refresh was refused, or the refreshed grant could not be stored.
    /// The ledger already carries the verdict; this is the sentence to log.
    Refused(Failure),
    /// No capability and no read grant produced a credential to look at, so
    /// nothing is known about the grant behind it.
    Unavailable(Failure),
}

/// Replace one subscription's access token once it has expired, so the stored
/// grant stays usable between requests.
///
/// The credential is read twice on the path that does refresh, and that is not
/// an oversight: the expiry has to be read before deciding, and the refresh
/// below deliberately re-reads under the rotation lock so a caller that waited
/// for a concurrent refresh observes what that refresh wrote instead of
/// rotating a grant the provider has already invalidated. Only a credential
/// that is genuinely due pays for the second read.
pub async fn refresh_subscription_credential_ahead(
    subscription_id: &str,
    provider: &str,
) -> RefreshAhead {
    let credential = match redeem_subscription_credential(subscription_id, provider).await {
        Ok(credential) => credential,
        Err(refused) => return RefreshAhead::Unavailable(refused),
    };
    // The account this grant belongs to, recorded while it is open, when the
    // item records none. A member imported before the account was recorded
    // beside its grant carries no account at all, so the pool can attribute
    // only the members this gateway has itself signed in and reports the
    // rest as unattributed, however exactly their own ids name an account.
    // This is the reusable repair: the sweep opens every member's credential
    // on every pass anyway, and the provider's own claim about it is what
    // gets written. A failed write is logged with the member and its error:
    // discarded, it left every burnt member without an account and every
    // automatic sign-in refused at `subscription_identity_missing` with
    // nothing on record saying why the account was never written.
    if let Err(error) =
        super::tags::record_stated_account(subscription_id, provider, &credential).await
    {
        warn!(
            event = "subscription_account_unrecorded",
            subscription = %subscription_id,
            provider = %provider,
            %error,
            "the account this grant states could not be recorded on its vault item"
        );
    }
    let expires_at_ms = oauth_refresh::access_token_expiry_ms(&credential, provider);
    // Ask what the document is before asking when it dies. A document that is
    // not a credential has no expiry either, and "no expiry" read as "nothing
    // to do": five of this fleet's accounts held sign-in metadata instead of a
    // grant, and every sweep for three days answered `NotDue` about them while
    // every request answered `no active credential`. The reduction is the
    // request path's own -- `credential_key`, the same call a dispatch makes
    // before it builds an authorization header -- so the sweep and a real
    // request cannot disagree about whether an account has a credential.
    if let Some(detail) = unusable_document(subscription_id, provider, &credential) {
        crate::subscription_dispatch::usage::record_reauthorization_needed(
            subscription_id,
            provider,
            &detail,
        );
        return RefreshAhead::Unavailable(
            failure::envelope(
                POINT_CREDENTIAL_PERSIST,
                Code::Config,
                IMPACT_CREDENTIAL_PERSIST,
                detail,
            )
            .with_context("subscription", subscription_id)
            .with_context("provider", provider),
        );
    }
    if !oauth_refresh::needs_refresh(&credential, provider) {
        return RefreshAhead::NotDue { expires_at_ms };
    }
    // Dropped before the refresh so this token is not held in memory across the
    // wait for the rotation lock and the provider's answer.
    drop(credential);
    match refresh_subscription_credential_inner(subscription_id, provider, true).await {
        Ok(refreshed) => RefreshAhead::Refreshed {
            expires_at_ms: oauth_refresh::access_token_expiry_ms(&refreshed, provider),
        },
        Err(refused) => RefreshAhead::Refused(refused),
    }
}
