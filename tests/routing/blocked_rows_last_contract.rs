//! A live account behind burnt ones is still tried.
//!
//! The walk takes at most two credentials per route and, until 2026-09-18,
//! took the first two rows of the ordered list before skipping the blocked
//! ones among them. A burnt subscription has no plan reading and no reading
//! sorts as the freest plan, so on charless-mac-mini two live Claude
//! accounts sat behind three burnt ones and every `best` call for `oko`
//! spent both attempts on burnt rows, walked nothing, and was refused
//! `all bounded 'claude-code' credentials were rejected by the provider;
//! re-authorization required` without one provider attempt. The live
//! accounts were never asked.
//!
//! Everything here is the product: the real gateway over a real isolated
//! Skarbiec vault, two accounts seeded as items whose document is not a
//! credential — which the request path records as needing a sign-in and
//! blocks — a third banked through the real pool API with a fabricated
//! grant, real signed requests, and the gateway's own ledger and log. The
//! fabricated grant reaches the provider, which refuses it; that refusal
//! recorded against the third account is the proof it was tried at all.

#[path = "../support/gateway.rs"]
mod gateway;
#[path = "../support/mod.rs"]
mod support;

use reqwest::Method;
use serde_json::{json, Value};

use gateway::{Gateway, AGENT, POOL};

/// A Codex grant in the shape the refresh path reads, carrying a refresh
/// token so the bank accepts it, and expiring far enough ahead that the
/// request path presents it as it stands.
fn fabricated_codex_grant() -> String {
    json!({
        "auth_mode": "chatgpt",
        "tokens": {
            "access_token": "blocked-rows-last-access-token",
            "refresh_token": "blocked-rows-last-refresh-token",
            "account_id": "blocked-rows-last-account",
        },
        "last_refresh": "2999-01-01T00:00:00Z",
    })
    .to_string()
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

fn ledger(gateway: &Gateway, answer: &Value) -> Value {
    serde_json::from_str(
        &std::fs::read_to_string(gateway.root().join("usage.json")).unwrap_or_else(|error| {
            panic!("the gateway writes its usage ledger ({error}); its answer was {answer}")
        }),
    )
    .expect("the ledger is JSON")
}

#[test]
fn a_live_account_behind_burnt_ones_is_still_tried() {
    let gateway = Gateway::start("blocked-rows-last", &[]);
    // Two accounts whose grant expired at the turn of the century with no
    // refresh token to ask the provider with: the request path records each
    // as needing a sign-in and blocks it, without a provider call.
    let expired =
        json!({"access_token": "expired-access-token", "expires_at": "2000-01-01T00:00:00Z"})
            .to_string();
    for id in ["burnt-one", "burnt-two"] {
        gateway
            .vault()
            .seed_subscription_with_document(AGENT, "codex", id, &expired);
    }

    let (first_status, first_body) = ranked_request(&gateway);
    assert_ne!(first_status, 200, "{first_body}");
    let burnt = ledger(&gateway, &first_body);
    for id in ["burnt-one", "burnt-two"] {
        assert_eq!(
            burnt["subscriptions"][id]["credential"]["state"], "needs_reauthorization",
            "{id} is burnt after the first request: {} (answer {first_body})",
            burnt["subscriptions"][id]
        );
    }

    // A third account joins with a grant the walk can present.
    let (status, receipt) = gateway.agent(
        POOL,
        Method::POST,
        Some(&json!({"action": "bank", "provider": "codex", "api_key": fabricated_codex_grant()})),
    );
    assert_eq!(status, 200, "{receipt}");
    let live = receipt["subscription"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("the receipt names the banked subscription: {receipt}"))
        .to_owned();

    // The walk must reach it: the provider's own refusal recorded against
    // this account is the proof, and an answer that attempted nothing is the
    // defect.
    let (second_status, second_body) = ranked_request(&gateway);
    assert_ne!(
        second_status, 200,
        "a fabricated grant is refused: {second_body}"
    );
    let attempts = second_body["error"]["attempts"]
        .as_u64()
        .unwrap_or_default();
    assert!(
        attempts > u64::MIN,
        "the live account was never tried: {second_body}"
    );
    let after = ledger(&gateway, &second_body);
    let credential = &after["subscriptions"][&live]["credential"];
    assert!(
        credential["cause"].as_str().is_some_and(|cause| !cause.is_empty()),
        "the provider's refusal of the live account is recorded, which means it was asked: {credential}"
    );
    let log = gateway.log();
    assert!(
        log.contains("credential_blocked"),
        "the burnt rows are still walked and named after the live one: {log}"
    );
}
