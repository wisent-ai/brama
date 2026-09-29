//! What a caller is told when a credential pool empties.
//!
//! ARCHITECTURE.md carries one rule about this: "An authorization failure must
//! never be dressed as capacity." It has been broken twice. The first time, a
//! refused capability redemption was answered `429 capacity_error,
//! retryable: true` while Skarbiec was saying `authorization id does not
//! match`, and the fix was recorded as `503 authorization_error`,
//! `credential_unauthorized`, `retryable: false`.
//!
//! The second time it arrived one layer further in, and cost a CI pipeline a
//! day. `codex` answered `401 Your session has ended. Please log in again`.
//! That records two things in the ledger: `needs_reauthorization`, which says a
//! sign-in is the repair, and a half-hour block, which stops the credential
//! being spent meanwhile. The router skips a blocked credential without asking
//! any provider, so for that half hour the pool emptied with nothing observed
//! and fell through to the capacity sentence -- `all bounded 'codex'
//! credentials unavailable for agent`, answered `429 retryable: true`. The
//! ledger had recorded the authorization failure the whole time.
//!
//! Nothing here calls a provider or spends anything: an emptied pool is decided
//! before any provider is asked, which is exactly why it can be pinned down
//! cheaply and why it went uncovered for so long.

#[path = "../support/mod.rs"]
mod support;

use std::process::Command;

use brama::subscription_dispatch::dispatch::{
    capacity_is_mixed, capacity_summary, pool_empty_summary, pool_is_capacity, PoolEmptyCause,
};
use support::{SkarbiecVault, TestDirectory};

const NOTHING_OBSERVED: PoolEmptyCause = PoolEmptyCause {
    auth_rejection: false,
    reauthorization_block: false,
    unredeemable_credential: false,
};

/// The whole chain for the failure that reopened this: a credential sitting in
/// an authorization block must reach the caller as an authorization failure.
#[test]
fn an_authorization_block_is_not_reported_as_capacity() {
    let cause = PoolEmptyCause {
        reauthorization_block: true,
        ..NOTHING_OBSERVED
    };
    assert!(cause.needs_authorization());

    let summary = pool_empty_summary("codex", cause);
    assert!(
        summary.contains("were rejected by the provider; automatic sign-in:"),
        "a blocked-for-authorization pool must say what the automatic sign-in did, said: {summary}"
    );
}

/// Both directions of this rule have cost a diagnosis, so both are pinned.
///
/// On 2026-09-09 production answered `429 all bounded 'codex' credentials
/// unavailable for agent`, retryable, while the ledger said every one of those
/// credentials needed a sign-in. That case records no rate-limit block at all,
/// because the walk reads the ledger per credential, so it is authorization.
///
/// On 2026-09-21 the opposite: the pool held one live credential at 100% of
/// its seven-day quota, resetting in fourteen hours, beside members burnt by
/// the borrowing this product removed. The answer was
/// `503 subscription_reauthorization_required` — a task for a person, while
/// the repair was a wait that the quota-blocked member would have served.
#[test]
fn a_quota_blocked_member_is_capacity_even_beside_members_needing_a_sign_in() {
    let mixed = PoolEmptyCause {
        reauthorization_block: true,
        ..NOTHING_OBSERVED
    };
    assert!(
        pool_is_capacity(true),
        "a credential blocked by quota alone is usable after the wait, whatever the other \
         members need"
    );
    assert!(
        capacity_is_mixed(mixed),
        "the sentence must say that other members need a sign-in"
    );
    assert!(
        !capacity_is_mixed(NOTHING_OBSERVED),
        "a pool whose every credential is merely out of quota has nothing to sign in"
    );
    assert!(
        !pool_is_capacity(false),
        "nothing blocked by quota is not capacity: there is no block to wait out, and a pool \
         whose every credential needs a sign-in records no quota block"
    );
}

/// A pool that is genuinely out of quota keeps the retryable capacity contract.
/// Without this the fix could be "call everything authorization", which would
/// stop callers retrying things that waiting really does repair.
#[test]
fn an_exhausted_pool_is_still_capacity() {
    let cause = NOTHING_OBSERVED;
    assert!(!cause.needs_authorization());

    let summary = pool_empty_summary("codex", cause);
    assert!(
        summary.contains("all bounded"),
        "an exhausted pool keeps its own sentence, said: {summary}"
    );
}

#[path = "refusal_contract/sentences.rs"]
mod sentences;
