//! A grant the provider disowned, and when that verdict was established.
//!
//! Recording the refusal is two writes at once, because the question "may this
//! credential be spent" and the question "what repairs it" have different
//! answers: a half-hour block stops it being presented meanwhile, and the state
//! says a sign-in rather than a wait is the repair. One of those was missing
//! for as long as it took to lose five days.
//!
//! The instant is the subtle part and is why this is its own file. It records
//! when the verdict was ESTABLISHED, not when it was last restated, because the
//! renewal sweep compares it against the browser sign-in already spent on the
//! credential. Restamping an identical refusal reads as new information and
//! buys one more real sign-in, so an identical refusal keeps the instant it
//! first got, and the tests below pin exactly that.

use crate::core::failure::{self, IMPACT_CREDENTIAL_BLOCK, POINT_CREDENTIAL_BLOCK};
use crate::subscription_dispatch::usage::{now_ms, with_ledger, Block, REASON_LIMIT};

use super::{Credential, CredentialState};

// Long enough that a renewal has a chance to run, short enough that a renewed
// credential is not gated by the record of the one it replaced.
const REAUTHORIZATION_BLOCK_MS: i64 = 30 * 60 * 1_000;

/// When a refusal's verdict was ESTABLISHED, given what the ledger already
/// held and the refusal being recorded now.
///
/// `recorded_at_ms` is not "when this was last restated", and the difference
/// is load-bearing: the renewal sweep compares it against the browser sign-in
/// already spent on the credential, so restamping an identical refusal reads
/// as new information and buys one more real sign-in. On 2026-09-02 a single
/// operator-forced model call -- which re-records the same refusal on its way
/// to failing -- was enough to reopen the loop the sweep gate had just closed.
///
/// A refusal whose state and provider sentence are unchanged keeps the instant
/// it was first established. Anything else is a new verdict and gets now: a
/// different sentence from the provider is a different statement about the
/// account, and a credential that had gone back to `active` in between has a
/// genuinely new refusal even if the sentence repeats.
fn refusal_recorded_at_ms(previous: Option<&Credential>, cause: &str, now: i64) -> i64 {
    match previous {
        Some(credential)
            if credential.state == CredentialState::NeedsReauthorization
                && credential.cause.as_deref() == Some(cause) =>
        {
            credential.recorded_at_ms
        }
        _ => now,
    }
}

/// Record that a credential can no longer be renewed on its own, because the
/// rotated grant was lost.
///
/// A provider that rotates refresh tokens invalidates the previous one the
/// moment it issues a new one. If that new grant is not written back, the copy
/// in the vault is already dead and every later use fails with `invalid_grant`
/// no matter how healthy the account is. This is not a rate limit and it is not
/// transient: it needs a re-authorization, and the window is short so that a
/// successful renewal is not gated by a stale record.
pub fn record_reauthorization_needed(subscription_id: &str, provider: &str, reason: &str) {
    let now = now_ms();
    let recorded = failure::envelope(
        POINT_CREDENTIAL_BLOCK,
        failure::code_for("credential_unauthorized"),
        IMPACT_CREDENTIAL_BLOCK,
        reason,
    )
    .with_context("subscription", subscription_id)
    .with_context("provider", provider);
    let cause: String = reason.chars().take(REASON_LIMIT).collect();
    with_ledger(|ledger| {
        let entry = ledger
            .subscriptions
            .entry(subscription_id.to_string())
            .or_default();
        entry.provider = provider.to_string();
        entry.updated_at_ms = Some(now);
        // The block stops the credential being spent for the next half hour;
        // the state is what says a sign-in, not a wait, is the repair. Both are
        // written because they answer different questions and one of them was
        // missing for as long as it took to lose five days.
        let expires_at_ms = entry
            .credential
            .as_ref()
            .and_then(|credential| credential.expires_at_ms);
        let refreshed_at_ms = entry
            .credential
            .as_ref()
            .and_then(|credential| credential.refreshed_at_ms);
        let previous = entry.credential.take();
        entry.credential = Some(Credential {
            state: CredentialState::NeedsReauthorization,
            cause: Some(cause.clone()),
            recorded_at_ms: refusal_recorded_at_ms(previous.as_ref(), &cause, now),
            expires_at_ms,
            refreshed_at_ms,
            borrowed_from: previous
                .as_ref()
                .and_then(|credential| credential.borrowed_from.clone()),
        });
        entry.block = Some(Block {
            blocked_until_ms: now.saturating_add(REAUTHORIZATION_BLOCK_MS),
            reason: cause,
            recorded_at_ms: now,
            envelope: Some(recorded.to_json()),
            quota_exhausted: false,
        });
    });
}
