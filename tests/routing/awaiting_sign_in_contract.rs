//! A grant the ledger says awaits a sign-in is left alone on the request path.
//!
//! The refresh sweep already trusts the ledger: a grant the provider disowned
//! is not presented again until a sign-in replaces it. The request path did
//! not. Every request that ranked an agent's subscriptions read each grant
//! from the vault -- a child process to the entitlements router and a broker
//! redemption -- and an expired grant then paid for a refresh the ledger had
//! already recorded as refused. On charless-mac-mini that was the codex
//! account, whose document has no refresh token, refused several times a
//! minute for days while the callers waited on it.
//!
//! Everything here is the product: the real gateway over a real isolated
//! Skarbiec vault, the account banked through the real pool API, real signed
//! requests, the gateway's own log and the ledger it wrote.

#[path = "../support/gateway.rs"]
mod gateway;
#[path = "../support/mod.rs"]
mod support;

use reqwest::Method;
use serde_json::{json, Value};

use gateway::{Gateway, AGENT, POOL};

/// An OAuth document the request path must refresh and cannot: expired at the
/// turn of the century, and carrying no refresh token to ask the provider
/// with.
fn expired_grant_without_refresh_token() -> String {
    json!({"access_token": "expired-access-token", "expires_at": "2000-01-01T00:00:00Z"})
        .to_string()
}

/// Bank one codex account for the agent through the pool API, as an operator
/// or a sign-in does, and return the subscription id the gateway minted.
fn bank_codex(gateway: &Gateway, grant: &str) -> String {
    let (status, receipt) = gateway.agent(
        POOL,
        Method::POST,
        Some(&json!({"action": "bank", "provider": "codex", "api_key": grant})),
    );
    assert_eq!(status, 200, "{receipt}");
    receipt["subscription"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("the receipt names the banked subscription: {receipt}"))
        .to_owned()
}

fn ranked_request(gateway: &Gateway) -> (u16, Value) {
    gateway.agent(
        "/v1/chat/completions",
        Method::POST,
        Some(&json!({
            "model": "best",
            "messages": [{"role": "user", "content": "hello"}],
        })),
    )
}

/// How many times an event was written so far.
fn occurrences(log: &str, event: &str) -> usize {
    log.matches(&format!("event=\"{event}\"")).count()
}

/// The `subscription_models_discovered` lines written so far, oldest first.
fn discoveries(log: &str) -> Vec<&str> {
    log.lines()
        .filter(|line| line.contains("event=\"subscription_models_discovered\""))
        .collect()
}

fn ledger(gateway: &Gateway) -> Value {
    serde_json::from_str(
        &std::fs::read_to_string(gateway.root().join("usage.json"))
            .expect("the gateway writes its usage ledger"),
    )
    .expect("the ledger is JSON")
}

#[test]
fn a_disowned_grant_is_not_read_again_until_a_sign_in_replaces_it() {
    let gateway = Gateway::start("awaiting-sign-in", &[]);
    let subscription = bank_codex(&gateway, &expired_grant_without_refresh_token());

    // The first request reads the grant, refreshes it, and is refused by the
    // document itself: no provider is contacted, because there is no refresh
    // token to contact one with. The ledger now says a sign-in is owed.
    let (first_status, first_body) = ranked_request(&gateway);
    assert_ne!(
        first_status, 200,
        "nothing can answer for a grant that cannot refresh: {first_body}"
    );
    let after_first = gateway.log();
    assert_eq!(
        occurrences(&after_first, "credential_refresh_refused_definitively"),
        1,
        "the first read is the one that learns the grant is dead:\n{after_first}"
    );
    let first_discovery = discoveries(&after_first);
    assert_eq!(
        first_discovery.len(),
        1,
        "one discovery per ranked request:\n{after_first}"
    );
    assert!(
        first_discovery[0].contains("read=1") && first_discovery[0].contains("awaiting_sign_in=0"),
        "the first discovery read the credential: {}",
        first_discovery[0]
    );
    let credential = ledger(&gateway)["subscriptions"][&subscription]["credential"].clone();
    assert_eq!(credential["state"], "needs_reauthorization", "{credential}");
    assert!(
        credential["cause"]
            .as_str()
            .is_some_and(|cause| cause.contains("no refresh token")),
        "the ledger names why: {credential}"
    );

    // The second request asks the ledger first and never reaches the grant:
    // no refresh, no refusal, and the discovery line says what it left alone.
    let (second_status, second_body) = ranked_request(&gateway);
    assert_ne!(second_status, 200, "the grant is still dead: {second_body}");
    let after_second = gateway.log();
    assert_eq!(
        occurrences(&after_second, "credential_refresh_refused_definitively"),
        1,
        "the disowned grant was presented again:\n{after_second}"
    );
    assert_eq!(
        occurrences(&after_second, "oauth_refresh_failed"),
        1,
        "the second request must not pay for a refresh the ledger already refused:\n{after_second}"
    );
    let second_discovery = discoveries(&after_second);
    assert_eq!(
        second_discovery.len(),
        2,
        "one discovery per ranked request:\n{after_second}"
    );
    assert!(
        second_discovery[1].contains("read=0")
            && second_discovery[1].contains("awaiting_sign_in=1"),
        "the second discovery must leave the grant alone: {}",
        second_discovery[1]
    );
}

/// A credential that cannot be obtained at all is remembered for the failure
/// window instead of being asked for on every request. Here the account is in
/// the vault but this gateway holds no broker to redeem it through, which is
/// the refusal a broken installation produces on every call.
#[test]
fn a_refused_credential_is_not_asked_for_again_within_the_failure_window() {
    let gateway = Gateway::start(
        "refused-credential-window",
        &[(AGENT, "codex", "unredeemable-codex")],
    );

    let (status, body) = ranked_request(&gateway);
    assert_ne!(status, 200, "{body}");
    let after_first = gateway.log();
    assert_eq!(
        occurrences(&after_first, "subscription_model_credential_failed"),
        1,
        "the refusal is logged once:\n{after_first}"
    );

    let (status, body) = ranked_request(&gateway);
    assert_ne!(status, 200, "{body}");
    let after_second = gateway.log();
    assert_eq!(
        occurrences(&after_second, "subscription_model_credential_failed"),
        1,
        "the second request must answer from the remembered refusal:\n{after_second}"
    );
    assert_eq!(discoveries(&after_second).len(), 2, "{after_second}");
}
