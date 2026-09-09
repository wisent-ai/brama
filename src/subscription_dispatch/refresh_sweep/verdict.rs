//! Whether another automatic browser sign-in could possibly answer differently
//! than the last one did.
//!
//! This is a decision about spending a real single-sign-on, not about timing,
//! and it is the one rule in this sweep that was learnt from an incident rather
//! than designed: the cooldown that preceded it made a repeated sign-in a rate
//! instead of a stop, and six days at that rate locked two accounts out of
//! their own authenticator. It stands apart from the code that drives the
//! browser so that the comparison is one readable expression sitting next to
//! the history that explains why it is the shape it is, and so that the tests
//! pinning it read as what they are -- a regression on that loop -- rather than
//! as tests of the sweep's bookkeeping.

/// Whether another automatic browser sign-in for this subscription could
/// possibly answer differently than the last one did.
///
/// Only one thing changes the outcome of a sign-in: the stored credential. The
/// ledger writes a new instant every time that changes -- a definitive refusal,
/// a rotation, a sign-in that stored something -- so a verdict no newer than
/// the newest completed sign-in proves a browser has already been driven
/// against exactly this state, and driving it again spends a real
/// single-sign-on to be told the same thing.
///
/// This gate exists because the cooldown alone was not one. On 2026-08-27
/// `brama-sub-wisent-app-codex-primary` was recorded `needs_reauthorization`
/// with the provider's own sentence, "Your session has ended. Please log in
/// again." Every sweep after that read the ledger, saw a state that is not
/// usable, and scheduled a browser sign-in; the half-hour cooldown made that a
/// rate, not a stop. By 2026-09-02 the fleet had driven more than a thousand
/// of them, three providers interleaved, one real Google login roughly every
/// ten minutes for six days. Each one resubmitted a code the account had
/// already rejected, until Google answered "Too many failed attempts" and
/// locked the authenticator method on two accounts -- so the loop did not
/// merely fail to repair the grant, it destroyed the operator's own ability to
/// repair it by hand. The ledger held the verdict that says a timer cannot fix
/// this; nothing read it.
pub fn verdict_outranks_last_sign_in(
    verdict_at_ms: Option<i64>,
    last_sign_in_at_ms: Option<i64>,
) -> bool {
    match (verdict_at_ms, last_sign_in_at_ms) {
        // Nothing has ever been signed in for this subscription, so the one
        // attempt an automatic repair gets has not been spent.
        (_, None) => true,
        // A verdict recorded after the last sign-in is new information: either
        // the stored credential changed, or the provider said something it had
        // not said before. Either way the answer can differ.
        (Some(verdict_at_ms), Some(signed_in_at_ms)) => verdict_at_ms > signed_in_at_ms,
        // A sign-in has completed and the ledger holds no verdict about the
        // credential at all -- the vault row yielded nothing then, and nothing
        // since has said otherwise. Another browser run reads the same row.
        (None, Some(_)) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::verdict_outranks_last_sign_in;

    /// Milliseconds for 2026-08-27T00:24:00Z, the instant the codex grant was
    /// recorded `needs_reauthorization` with "Your session has ended. Please
    /// log in again."
    const REFUSAL_RECORDED_MS: i64 = 1_787_790_240_000;

    /// The defect this gate exists for: a definitively-refused credential that
    /// a browser sign-in has already been driven against must never be driven
    /// again by the sweep, however much later the sweep runs.
    #[test]
    fn a_refusal_already_signed_in_against_is_never_re_driven() {
        let first_attempt = REFUSAL_RECORDED_MS + 60_000;
        assert!(
            !verdict_outranks_last_sign_in(Some(REFUSAL_RECORDED_MS), Some(first_attempt)),
            "the sweep re-drove a browser sign-in for a credential whose recorded refusal \
             predates the sign-in already spent on it"
        );
        // Six days later, which is how long the real loop ran.
        let six_days_later = first_attempt + 6 * 24 * 60 * 60 * 1000;
        assert!(
            !verdict_outranks_last_sign_in(Some(REFUSAL_RECORDED_MS), Some(six_days_later)),
            "elapsed time is not new information about the stored credential"
        );
    }

    /// A vault row that yields no credential at all is the same wall: the
    /// ledger holds no verdict, and a sign-in has already read that row.
    #[test]
    fn a_credential_less_row_already_signed_in_against_is_never_re_driven() {
        assert!(!verdict_outranks_last_sign_in(
            None,
            Some(REFUSAL_RECORDED_MS)
        ));
    }

    /// The one automatic attempt still happens. A subscription nothing has ever
    /// signed in gets its browser run, whatever the ledger says.
    #[test]
    fn the_first_automatic_sign_in_is_allowed() {
        assert!(verdict_outranks_last_sign_in(
            Some(REFUSAL_RECORDED_MS),
            None
        ));
        assert!(verdict_outranks_last_sign_in(None, None));
    }

    /// A refusal recorded after the last sign-in is new information: the grant
    /// worked, then died, and that is exactly the case an automatic sign-in
    /// repairs without waking anybody.
    #[test]
    fn a_refusal_newer_than_the_last_sign_in_is_driven() {
        let earlier_sign_in = REFUSAL_RECORDED_MS - 60_000;
        assert!(verdict_outranks_last_sign_in(
            Some(REFUSAL_RECORDED_MS),
            Some(earlier_sign_in)
        ));
    }
}
