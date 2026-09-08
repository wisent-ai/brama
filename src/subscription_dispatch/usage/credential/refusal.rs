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
        });
        entry.block = Some(Block {
            blocked_until_ms: now.saturating_add(REAUTHORIZATION_BLOCK_MS),
            reason: cause,
            recorded_at_ms: now,
            envelope: Some(recorded.to_json()),
        });
    });
}

#[cfg(test)]
mod tests {
    use super::{refusal_recorded_at_ms, Credential, CredentialState};

    /// 2026-08-27T00:24:00Z, when codex's grant was first refused.
    const ESTABLISHED_MS: i64 = 1_787_790_240_000;
    const SENTENCE: &str = "OAuth refresh rejected with HTTP 401: Your session has ended. \
                            Please log in again.";

    fn refused(cause: &str, recorded_at_ms: i64) -> Credential {
        Credential {
            state: CredentialState::NeedsReauthorization,
            cause: Some(cause.to_string()),
            recorded_at_ms,
            expires_at_ms: None,
            refreshed_at_ms: None,
        }
    }

    /// The defect: re-recording the same refusal restamped the verdict, which
    /// the sweep reads as new information and answers with a browser sign-in.
    #[test]
    fn restating_an_identical_refusal_keeps_the_original_instant() {
        let previous = refused(SENTENCE, ESTABLISHED_MS);
        let much_later = ESTABLISHED_MS + 6 * 24 * 60 * 60 * 1000;
        assert_eq!(
            refusal_recorded_at_ms(Some(&previous), SENTENCE, much_later),
            ESTABLISHED_MS,
            "an identical refusal restated later is not a new verdict"
        );
    }

    /// A different sentence from the provider IS a new statement about the
    /// account, and one automatic sign-in against it is the design.
    #[test]
    fn a_different_provider_sentence_is_a_new_verdict() {
        let previous = refused(SENTENCE, ESTABLISHED_MS);
        let now = ESTABLISHED_MS + 1000;
        assert_eq!(
            refusal_recorded_at_ms(Some(&previous), "invalid_grant", now),
            now
        );
    }

    /// A credential that had gone back to active has a genuinely new refusal
    /// even when the sentence repeats.
    #[test]
    fn a_refusal_after_a_working_credential_is_a_new_verdict() {
        let previous = Credential {
            state: CredentialState::Active,
            cause: None,
            recorded_at_ms: ESTABLISHED_MS,
            expires_at_ms: None,
            refreshed_at_ms: None,
        };
        let now = ESTABLISHED_MS + 1000;
        assert_eq!(refusal_recorded_at_ms(Some(&previous), SENTENCE, now), now);
    }

    /// The first refusal ever recorded establishes the verdict.
    #[test]
    fn a_first_refusal_establishes_the_verdict() {
        assert_eq!(
            refusal_recorded_at_ms(None, SENTENCE, ESTABLISHED_MS),
            ESTABLISHED_MS
        );
    }
}
