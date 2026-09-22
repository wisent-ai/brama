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
use std::time::Duration;

use tracing::warn;

use super::super::standalone::{
    local_provider_credentials_enabled, put_local_subscription_credential,
};
use super::super::vault::{existing_item_account, existing_item_tags, put_credential};
use super::super::{redeem_subscription_credential, slug};
use super::tags::subscription_tags_for_write;
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
    // Refresh-token rotation is single-flight. Re-reading after the lock lets a
    // concurrent caller observe the value already written to the vault.
    let _guard = OAUTH_REFRESH_LOCK.lock().await;
    let credential = redeem_subscription_credential(subscription_id, provider).await?;
    // A grant taken from a harness is the harness's to rotate. Rotating it
    // here revokes the refresh token the harness holds: on 2026-09-17 two
    // accounts `omp` was signed into died within the hour of Brama's first
    // refresh of their copies, one of them under the operator's own open
    // session. So a borrowed grant is spent as it stands - its expiry is a
    // hint, and the provider is the one to say no - and when the provider
    // has said no (`force`: the request path asking for a rotation after a
    // refusal, or an operator's refresh), the member is disowned with the
    // harness named, and the sweep brings the harness's current grant.
    let borrowed = borrowed_from(&credential);
    if !force && (borrowed.is_some() || !oauth_refresh::needs_refresh(&credential, provider)) {
        return Ok(credential);
    }
    if let Some(harness) = borrowed {
        let refused = failure::envelope(
            POINT_CREDENTIAL_PERSIST,
            Code::Config,
            IMPACT_CREDENTIAL_PERSIST,
            format!(
                "this grant is borrowed from {harness}, which refreshes it itself; Brama does not \
                 rotate it, and the sweep brings {harness}'s current grant"
            ),
        )
        .with_context("subscription", subscription_id)
        .with_context("provider", provider);
        crate::subscription_dispatch::usage::record_reauthorization_needed(
            subscription_id,
            provider,
            refused.detail.as_deref().unwrap_or_default(),
        );
        return Err(refused);
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
                error = refused.detail.as_deref().unwrap_or_default(),
                envelope = %refused.to_json(),
                "OAuth refresh failed"
            );
            record_refusal(subscription_id, provider, &refused);
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
            error = unpersisted.detail.as_deref().unwrap_or_default(),
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

/// Put a refused refresh in the ledger when the provider disowned the grant,
/// and leave the record alone when nothing about the grant was learned.
///
/// Both refresh paths classify here -- the sweep ahead of expiry and the forced
/// refresh a rejected request triggers -- so a credential cannot read as dead on
/// one path and healthy on the other.
fn record_refusal(subscription_id: &str, provider: &str, refused: &Failure) {
    // The provider's own sentence, or a stand-in when it refused without one:
    // the ledger's cause is what an operator reads, and an empty cause is a row
    // that says a sign-in is needed without saying why.
    let detail = refused
        .detail
        .as_deref()
        .unwrap_or("the provider refused this credential's refresh without saying why");
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


mod storing;

pub use storing::{
    put_subscription_credential, put_subscription_credential_for_account, subscription_account,
};
use storing::{borrowed_from, unusable_document};

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

/// Whether this provider's subscription credentials are OAuth grants that can be
/// refreshed, rather than API keys that never expire.
pub fn supports_oauth_refresh(provider: &str) -> bool {
    oauth_refresh::supports_refresh(provider)
}

/// What one refresh-ahead attempt concluded, for a caller that never holds the
/// credential itself.
pub enum RefreshAhead {
    /// This grant has more than the skew window left, or the provider is not one
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

/// Why this stored document could not be presented to a provider, or nothing
/// when it can be.
/// that is genuinely due pays for the second read.
pub async fn refresh_subscription_credential_ahead(
    subscription_id: &str,
    provider: &str,
    skew: Duration,
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
    // gets written.
    let _ = super::tags::record_stated_account(subscription_id, provider, &credential).await;
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
    if !oauth_refresh::expires_within(&credential, provider, skew) {
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
