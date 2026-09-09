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

use axum::http::StatusCode;
use brama::core::server::model_error_contract;
use brama::subscription_dispatch::dispatch::{
    pool_empty_summary, pool_is_capacity, PoolEmptyCause,
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
        summary.contains("re-authorization required"),
        "a blocked-for-authorization pool must say so, said: {summary}"
    );

    // The sentence the dispatcher produces, classified the way the HTTP edge
    // classifies it. Asserted together because the defect lived in the seam:
    // each half was defensible alone.
    let contract = model_error_contract(&summary);
    assert_eq!(contract.status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(contract.error_type, "authorization_error");
    assert_eq!(contract.code, "subscription_reauthorization_required");
    assert!(
        !contract.retryable,
        "no wait reaches a credential the provider has already refused"
    );
}

/// The same sentence wrapped in the aggregate context a real multi-provider
/// walk produces. This is the exact shape the Kronika documentation gate was
/// answered with, and the classification must survive the wrapping.
#[test]
fn the_aggregate_refusal_keeps_the_authorization_classification() {
    let summary = pool_empty_summary(
        "codex",
        PoolEmptyCause {
            reauthorization_block: true,
            ..NOTHING_OBSERVED
        },
    );
    let aggregate =
        format!("no working subscription model for signed agent; codex refused ({summary})");

    let contract = model_error_contract(&aggregate);
    assert_eq!(contract.status, StatusCode::SERVICE_UNAVAILABLE);
    assert!(
        !contract.retryable,
        "the aggregate must not turn a sign-in into a retry: {aggregate}"
    );
}

/// The fourth time, and the one production answered on 2026-09-09 after the
/// gateway started recording sign-in-needed for accounts whose stored document
/// is not a credential: `429 all bounded 'codex' credentials unavailable for
/// agent`, retryable, while the ledger said every one of those credentials
/// needed a sign-in. Both branches were right on their own; the capacity one
/// was simply checked first.
#[test]
fn a_mixed_pool_is_authorization_not_capacity() {
    let mixed = PoolEmptyCause {
        reauthorization_block: true,
        ..NOTHING_OBSERVED
    };
    assert!(
        !pool_is_capacity(mixed, true),
        "a pool holding one rate-limited credential and one that needs a sign-in is not \
         capacity: no wait reaches the second"
    );
    assert!(
        !pool_is_capacity(
            PoolEmptyCause {
                unredeemable_credential: true,
                ..NOTHING_OBSERVED
            },
            true
        ),
        "a vault that produced no credential is not capacity either"
    );
    assert!(
        pool_is_capacity(NOTHING_OBSERVED, true),
        "a pool whose every credential is inside a rate-limit block is capacity, and a \
         caller that waits for it gets served"
    );
    assert!(
        !pool_is_capacity(NOTHING_OBSERVED, false),
        "nothing observed at all is not capacity: there is no block to wait out"
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

    let contract = model_error_contract(&summary);
    assert_eq!(contract.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(contract.error_type, "capacity_error");
    assert_eq!(contract.code, "subscription_unavailable");
    assert!(
        contract.retryable,
        "a recorded rate-limit block does expire, so this one is worth retrying"
    );
}

/// A vault that produced no credential is an authorization failure too, and was
/// already classified as one. Kept here so the arm cannot be lost while the
/// neighbouring ones are edited.
#[test]
fn a_pool_that_produced_no_credential_is_an_authorization_failure() {
    let summary = pool_empty_summary(
        "codex",
        PoolEmptyCause {
            unredeemable_credential: true,
            ..NOTHING_OBSERVED
        },
    );

    let contract = model_error_contract(&summary);
    assert_eq!(contract.status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(contract.code, "credential_unauthorized");
    assert!(!contract.retryable);
}

/// An agent with no eligible row at all -- the answer a subscription whose
/// vault item lost its `brama:agent:` tag produces, and the answer a retired
/// subscription produces. A missing tag is not capacity: no wait restores it.
#[test]
fn no_active_credential_is_an_authorization_failure() {
    let contract = model_error_contract("no active 'codex' credential for agent");
    assert_eq!(contract.status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(contract.error_type, "authorization_error");
    assert_eq!(contract.code, "credential_unauthorized");
    assert!(
        !contract.retryable,
        "a subscription that discovery cannot see does not appear by waiting"
    );
}

/// A named credential that is not active is the same fault with a pin on it.
#[test]
fn a_named_inactive_credential_is_an_authorization_failure() {
    let contract = model_error_contract(
        "selected credential 'brama-sub-wisent-app-codex-primary' is not active for provider 'codex' and agent",
    );
    assert_eq!(contract.status, StatusCode::SERVICE_UNAVAILABLE);
    assert!(!contract.retryable);
}

/// The narrowing that made the two arms above possible has to stay narrow. This
/// sentence also begins "no active", and it is about the model catalogue rather
/// than a credential, so it must not be pulled into the credential arm.
#[test]
fn a_catalogue_answer_is_not_mistaken_for_a_credential_answer() {
    let contract = model_error_contract("no active stateless provider models for signed agent");
    assert_eq!(
        contract.code, "subscription_unavailable",
        "a catalogue sentence must keep its own classification"
    );
}

/// The first version of this defect, kept so the original fix cannot be lost
/// while the new arms are edited beside it.
#[test]
fn a_refused_redemption_is_still_an_authorization_failure() {
    for message in [
        "capability is not issued, has expired, has no uses left, or its authorization id does not match",
        "capability redemption denied",
    ] {
        let contract = model_error_contract(message);
        assert_eq!(
            contract.status,
            StatusCode::SERVICE_UNAVAILABLE,
            "refused redemption must not be capacity: {message}"
        );
        assert_eq!(contract.code, "credential_unauthorized");
        assert!(!contract.retryable);
    }
}

/// The third time, and the reason this file gained a test that runs the
/// product: every arm above passed while the router was still telling callers
/// to retry. They read `model_error_contract`, which is the HTTP edge's
/// reading of a refusal sentence. The dispatch writes an envelope of its own,
/// and for an agent that holds no account it wrote
/// `"error_code":"rate_limit","retryable":true`, because the refusal reached
/// for the default kind `subscription_unavailable`. On 2026-09-09 `jeden run`
/// took that advice, retried twice against `codex/gpt-6-astra` and ended with
/// `model stream first-event timeout`, while `brama test` on the same route
/// had already said `no active 'codex' credential for agent`.
///
/// So this one drives the built binary over a real Skarbiec vault holding one
/// agent's account and asks for the route as a different agent. Free to run:
/// the roster is empty before any provider is asked.
#[test]
fn the_dispatch_envelope_agrees_with_the_edge_about_an_absent_account() {
    let directory = TestDirectory::new("envelope-no-account");
    let vault = SkarbiecVault::create("envelope-no-account");
    vault.seed_subscription("brama-envelope-owner", "codex", "envelope-owner-codex");

    let mut command = Command::new(env!("CARGO_BIN_EXE_brama"));
    for (name, value) in vault.environment() {
        command.env(name, value);
    }
    let refused = command
        .env("HOME", directory.path().join("home"))
        .env("XDG_STATE_HOME", directory.path().join("xdg-state"))
        .env("BRAMA_STATE_DIR", directory.path().join("state"))
        .env(
            "BRAMA_SUBSCRIPTION_USAGE_FILE",
            directory.path().join("usage.json"),
        )
        .env("BRAMA_PERF_PATH", directory.path().join("perf.json"))
        .env("ENTITLEMENTS_ROUTER_BIN", vault.router())
        .args([
            "test",
            "--model",
            "codex/gpt-6-astra",
            "--agent-id",
            "brama-envelope-stranger",
            "--allow-provider-cost",
        ])
        .output()
        .expect("brama test for an agent with no account");

    let said = String::from_utf8_lossy(&refused.stderr);
    assert!(
        !refused.status.success(),
        "a call nothing can pay for must fail:\n{said}"
    );
    assert!(
        said.contains("no active 'codex' credential for agent"),
        "the refusal must name the provider and the agent:\n{said}"
    );
    assert!(
        said.contains(r#""failure_point":"brama.dispatch.credential-selection""#),
        "the envelope must say where the call broke:\n{said}"
    );
    assert!(
        said.contains(r#""error_code":"auth""#) && said.contains(r#""retryable":false"#),
        "an absent account is an authorization failure no wait repairs:\n{said}"
    );
    assert!(
        !said.contains(r#""error_code":"rate_limit""#),
        "the retry advice is back, and a caller will spend its attempts on it:\n{said}"
    );
}
