//! Which recorded sign-in failures say nothing about what the provider did.

/// Whether a recorded sign-in failure says nothing about what the provider
/// did: the exchange with Weles died before an answer, the answer was
/// unreadable, or the run was cancelled (`weles runs cancel`), so the provider
/// was asked nothing that could be held against the account. The next
/// maintenance pass runs such a sign-in again, and it is no reason to ask an
/// operator for anything. These are the failure codes the sign-in itself
/// records, not a reading of their sentences.
pub fn effect_unknown(code: &str) -> bool {
    matches!(
        code,
        "weles_execution_unconfirmed" | "authentication_response_invalid" | "run_cancelled"
    )
}
