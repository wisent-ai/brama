//! The sentences one refusal reaches a caller as: which of them are an
//! authorization failure, which are capacity, and which must never be read as
//! a credential answer at all.

use axum::http::StatusCode;
use brama::core::server::model_error_contract;

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
/// So this one drives the built binary over a real Skarbiec vault and asks
/// for a route no account in the pool can pay for. Until 2026-09-16 that was
/// another agent's account; since every account serves every caller, it is
/// an account for another provider. Free to run: the roster is empty before
/// any provider is asked.
#[test]
fn the_dispatch_envelope_agrees_with_the_edge_about_an_absent_account() {
    let directory = TestDirectory::new("envelope-no-account");
    let vault = SkarbiecVault::create("envelope-no-account");
    vault.seed_subscription(
        "brama-envelope-owner",
        "claude-code",
        "envelope-owner-claude",
    );

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

/// A wait has to name the hour it ends.
///
/// The capacity sentence told the caller the quota "lifts on its own" and
/// sent them to `brama subscriptions`, which prints a block's reason and not
/// its end. On 2026-09-21 that left Oko's judge refused all day with no
/// instant anybody could name, while the ledger had held `blocked_until_ms`
/// since the block was recorded.
#[test]
fn a_capacity_refusal_names_the_instant_the_block_lifts() {
    let lifts_at = 1_790_000_000_000_i64; // 2026-09-21T14:13:20Z
    let named = capacity_summary("codex", true, Some(lifts_at));
    assert!(
        named.contains("the block lifts at 2026-09-21T14:13:20Z"),
        "the wait must name its end: {named}"
    );
    assert!(
        named.contains("need a sign-in"),
        "and still say what the other members need: {named}"
    );

    let unknown = capacity_summary("codex", false, None);
    assert!(
        !unknown.contains("the block lifts at"),
        "an instant the ledger does not hold is not invented: {unknown}"
    );

    // The classification the HTTP edge gives it must not change because the
    // sentence grew: a wait stays a retryable capacity answer.
    let contract = model_error_contract(&format!(
        "no working subscription model for signed agent; codex refused ({named})"
    ));
    assert_eq!(contract.status, StatusCode::TOO_MANY_REQUESTS, "{named}");
    assert!(contract.retryable, "{named}");
}
