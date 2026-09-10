//! Where one subscription's grant stands with its provider, and the three
//! things that move it.
//!
//! This is deliberately not a block. A block is a quota the provider hands back
//! on its own schedule; this is whether the credential is accepted at all, and
//! a refused grant recorded only as a block reads as a rate limit that never
//! clears. That confusion is what let a burnt grant look like an account having
//! a quiet week.
//!
//! A grant is confirmed working, replaced by a sign-in, or retired, and each of
//! those is written here together with what it implies about the refusal that
//! came before it. When a refusal's verdict was established is a separate
//! question with its own consequences and lives in `refusal`; what the ledger
//! alone can tell a renewal sweep before anything reads the vault lives in
//! `refresh_hint`.

mod refresh_hint;
mod refusal;

use serde::{Deserialize, Serialize};

use super::{now_ms, read_ledger, with_ledger, REASON_LIMIT};

pub use refresh_hint::{
    awaiting_sign_in_cause, credential_recorded_at_ms, credential_refresh_hint, RefreshHint,
};
pub use refusal::record_reauthorization_needed;

/// Where one subscription's credential stands with its provider.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CredentialState {
    /// Nothing has refused this grant, as far as anything has observed.
    #[default]
    Active,
    /// A refresh was refused definitively. No retry repairs this one; only a
    /// sign-in that replaces the stored grant does.
    NeedsReauthorization,
    /// An operator or a lifecycle retired this subscription.
    Disabled,
}

impl CredentialState {
    /// The stored name, which is also the name every reader sees.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::NeedsReauthorization => "needs_reauthorization",
            Self::Disabled => "disabled",
        }
    }

    /// Whether a credential in this state is worth presenting to a provider.
    pub fn usable(self) -> bool {
        matches!(self, Self::Active)
    }
}

/// What is known about one subscription's grant.
///
/// Every field is defaulted on read, because a ledger written before this
/// record existed is the normal case on the first start after an upgrade and
/// must keep loading.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Credential {
    #[serde(default)]
    pub state: CredentialState,
    /// The provider's own sentence for a refusal, trimmed like every other
    /// stored reason here. Absent while the credential is accepted, because a
    /// stale refusal beside a working grant is what sends an operator looking
    /// for a sign-in that is not needed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cause: Option<String>,
    /// When this state was established.
    #[serde(default)]
    pub recorded_at_ms: i64,
    /// When the provider says the access token stops working, when the
    /// credential states it at all. An API key states nothing and stays absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at_ms: Option<i64>,
    /// When a refresh last replaced this grant. Reading a still-valid token
    /// does not move it: the question it answers is when the last rotation
    /// happened, not when anything last looked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refreshed_at_ms: Option<i64>,
}

/// Record that this subscription's grant is in working order, and until when.
///
/// `refreshed` separates a grant this call replaced from one it only read.
/// Both prove the provider still accepts the credential, but only the first is
/// a rotation, and an operator asking why a token died early needs to know
/// which of the two the instant describes.
///
/// A call that changes nothing leaves the record untouched, including the
/// record's own `updated_at_ms`. A sweep that confirms what the ledger already
/// says has observed nothing new, and stamping it every minute would make every
/// row look freshly seen and leave no reading ever stale.
pub fn record_credential_active(
    subscription_id: &str,
    provider: &str,
    expires_at_ms: Option<i64>,
    refreshed: bool,
) {
    let now = now_ms();
    with_ledger(|ledger| {
        let entry = ledger
            .subscriptions
            .entry(subscription_id.to_string())
            .or_default();
        let previous = entry.credential.take();
        let unchanged = !refreshed
            && previous.as_ref().is_some_and(|credential| {
                credential.state == CredentialState::Active
                    && credential.cause.is_none()
                    && credential.expires_at_ms == expires_at_ms
            });
        if unchanged {
            entry.credential = previous;
            return;
        }
        // A grant that just refreshed is proof the provider accepts it, so the
        // half-hour block a refusal left behind has been outlived by evidence.
        // Only that block is cleared: a rate limit is the provider's own
        // schedule and is none of this record's business.
        if previous
            .as_ref()
            .is_some_and(|credential| credential.state == CredentialState::NeedsReauthorization)
        {
            entry.block = None;
        }
        entry.provider = provider.to_string();
        entry.updated_at_ms = Some(now);
        entry.credential = Some(Credential {
            state: CredentialState::Active,
            cause: None,
            recorded_at_ms: now,
            expires_at_ms,
            refreshed_at_ms: if refreshed {
                Some(now)
            } else {
                previous.and_then(|credential| credential.refreshed_at_ms)
            },
        });
    });
}

/// Record that a sign-in stored a new credential for this subscription.
///
/// This is the only thing that repairs a `needs_reauthorization`, so it clears
/// the cause and the block the refusal left. The previous grant's expiry and
/// rotation instants are dropped rather than kept: they describe a credential
/// that is no longer the one in the vault.
pub fn record_credential_signed_in(subscription_id: &str, provider: &str) {
    let now = now_ms();
    with_ledger(|ledger| {
        let entry = ledger
            .subscriptions
            .entry(subscription_id.to_string())
            .or_default();
        if entry
            .credential
            .as_ref()
            .is_some_and(|credential| credential.state == CredentialState::NeedsReauthorization)
        {
            entry.block = None;
        }
        entry.provider = provider.to_string();
        entry.updated_at_ms = Some(now);
        entry.credential = Some(Credential {
            state: CredentialState::Active,
            cause: None,
            recorded_at_ms: now,
            expires_at_ms: None,
            refreshed_at_ms: None,
        });
    });
}

/// Record that this subscription was retired, with the reason it was.
pub fn record_credential_disabled(subscription_id: &str, provider: &str, cause: &str) {
    let now = now_ms();
    with_ledger(|ledger| {
        let entry = ledger
            .subscriptions
            .entry(subscription_id.to_string())
            .or_default();
        let expires_at_ms = entry
            .credential
            .as_ref()
            .and_then(|credential| credential.expires_at_ms);
        let refreshed_at_ms = entry
            .credential
            .as_ref()
            .and_then(|credential| credential.refreshed_at_ms);
        entry.provider = provider.to_string();
        entry.updated_at_ms = Some(now);
        entry.credential = Some(Credential {
            state: CredentialState::Disabled,
            cause: Some(cause.chars().take(REASON_LIMIT).collect()),
            recorded_at_ms: now,
            expires_at_ms,
            refreshed_at_ms,
        });
    });
}

/// Whether this subscription's recorded block is an authorization block.
///
/// [`record_reauthorization_needed`] writes two things: the state that says a
/// sign-in is the repair, and a half-hour block that stops the credential being
/// spent meanwhile. The router skips a blocked credential without asking the
/// provider, so for that half hour the only record of *why* the pool is empty
/// is this state - and a caller told the pool was merely bounded is told to
/// wait for something no wait reaches. This is how the request path reads the
/// difference.
pub fn needs_reauthorization(subscription_id: &str) -> bool {
    read_ledger(|ledger| {
        ledger
            .subscriptions
            .get(subscription_id)
            .and_then(|entry| entry.credential.as_ref())
            .is_some_and(|credential| credential.state == CredentialState::NeedsReauthorization)
    })
}
