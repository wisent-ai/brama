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

use gateway::{Gateway, POOL};

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

fn ledger(gateway: &Gateway) -> Value {
    serde_json::from_str(
        &std::fs::read_to_string(gateway.root().join("usage.json"))
            .expect("the gateway writes its usage ledger"),
    )
    .expect("the ledger is JSON")
}

#[test]
fn a_disowned_grant_keeps_its_refusal_until_a_sign_in_replaces_it() {
    let gateway = Gateway::start("awaiting-sign-in", &[]);
    let subscription = bank_codex(&gateway, &expired_grant_without_refresh_token());
    let (first_status, first_body) = ranked_request(&gateway);
    assert_ne!(first_status, 200, "{first_body}");
    let credential = ledger(&gateway)["subscriptions"][&subscription]["credential"].clone();
    assert_eq!(credential["state"], "needs_reauthorization", "{credential}");
    assert!(
        credential["cause"]
            .as_str()
            .is_some_and(|cause| cause.contains("no refresh token")),
        "{credential}"
    );

    let (second_status, second_body) = ranked_request(&gateway);
    assert_ne!(second_status, 200, "{second_body}");
    assert_eq!(
        ledger(&gateway)["subscriptions"][&subscription]["credential"],
        credential,
        "a repeated request must not establish a new credential refusal"
    );

    let replaced = bank_codex(&gateway, &expired_grant_without_refresh_token());
    assert_eq!(replaced, subscription);
    let replacement = ledger(&gateway)["subscriptions"][&subscription]["credential"].clone();
    assert_eq!(replacement["state"], "active", "{replacement}");
    assert!(replacement["cause"].is_null(), "{replacement}");
}
