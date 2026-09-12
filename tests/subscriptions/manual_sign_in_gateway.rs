//! The manual Claude sign-in as Brama Desktop drives it: two requests to a
//! real gateway over a real vault, the verifier never leaving the gateway.
//!
//! Only the console may begin one; a signed agent is refused. The page the
//! console gets is the harness's page for the account it named; a paste that
//! carries another sign-in's state is refused by name and the sign-in is
//! spent; an id nobody began answers 404. The login itself is the operator's
//! own browser and account, and is not reproduced here.

#[path = "../support/gateway.rs"]
mod gateway;
#[path = "../support/mod.rs"]
mod support;

use reqwest::Method;
use serde_json::json;

use gateway::{refusal, Gateway, AGENT, OTHER_AGENT};

const BEGIN: &str = "/v1/admin/subscription-pool/sign-in-manual";

/// The accounts the vault holds: a Claude account and a codex one, so the
/// refusal for a provider without a manual flow is about a real row.
const VAULT: &[(&str, &str, &str)] = &[
    (AGENT, "claude-code", "pool-agent-claude"),
    (OTHER_AGENT, "codex", "pool-other-codex"),
];

#[test]
fn the_console_begins_a_sign_in_and_gets_the_harness_page() {
    let gateway = Gateway::start("manual-sign-in-begin", VAULT);
    let (status, body) = gateway.console(
        BEGIN,
        Method::POST,
        Some(&json!({"subscription_id": "pool-agent-claude", "reason": "story"})),
    );
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["provider"], "claude-code");
    assert_eq!(body["subscription_id"], "pool-agent-claude");
    let sign_in_id = body["sign_in_id"].as_str().expect("a sign-in id");
    let url = url::Url::parse(body["url"].as_str().expect("a page to open")).expect("a URL");
    assert_eq!(url.host_str(), Some("claude.ai"));
    let query: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();
    assert_eq!(query["client_id"], "9d1c250a-e61b-44d9-88ed-5944d1962f5e");
    assert_eq!(query["redirect_uri"], "http://localhost:54545/callback");
    assert_eq!(query["code_challenge_method"], "S256");
    assert_eq!(
        query["state"], sign_in_id,
        "the sign-in id is the state the page carries, so a paste can be matched to it"
    );
    assert!(body["expires_in_secs"]
        .as_u64()
        .is_some_and(|secs| secs > 0));
}

#[test]
fn a_signed_agent_may_not_begin_one() {
    let gateway = Gateway::start("manual-sign-in-agent", VAULT);
    let (status, body) = gateway.agent(
        BEGIN,
        Method::POST,
        Some(&json!({"subscription_id": "pool-agent-claude", "reason": "story"})),
    );
    assert_eq!(status, 403, "{body}");
}

#[test]
fn a_provider_without_a_manual_flow_is_refused_by_name() {
    let gateway = Gateway::start("manual-sign-in-codex", VAULT);
    let (status, body) = gateway.console(
        BEGIN,
        Method::POST,
        Some(&json!({"subscription_id": "pool-other-codex", "reason": "story"})),
    );
    assert_eq!(status, 409, "{body}");
    let (_, _, message) = refusal(&body);
    assert!(message.contains("`codex` is not it"), "{message}");
}

#[test]
fn an_unknown_account_and_a_missing_reason_are_refused() {
    let gateway = Gateway::start("manual-sign-in-refusals", VAULT);
    let (status, body) = gateway.console(
        BEGIN,
        Method::POST,
        Some(&json!({"subscription_id": "nobody-has-this", "reason": "story"})),
    );
    assert_eq!(status, 404, "{body}");
    let (status, body) = gateway.console(
        BEGIN,
        Method::POST,
        Some(&json!({"subscription_id": "pool-agent-claude", "reason": " "})),
    );
    assert_eq!(status, 400, "{body}");
    let (_, _, message) = refusal(&body);
    assert!(message.contains("--reason must say why"), "{message}");
}

/// A paste with a foreign state is refused, and the sign-in it was pasted
/// into is spent: the next paste to the same id is told to begin again.
#[test]
fn a_foreign_state_spends_the_sign_in() {
    let gateway = Gateway::start("manual-sign-in-foreign", VAULT);
    let (_, begun) = gateway.console(
        BEGIN,
        Method::POST,
        Some(&json!({"subscription_id": "pool-agent-claude", "reason": "story"})),
    );
    let sign_in_id = begun["sign_in_id"].as_str().expect("a sign-in id");
    let complete = format!("{BEGIN}/{sign_in_id}");
    let (status, body) = gateway.console(
        &complete,
        Method::POST,
        Some(&json!({"code": "some-code#somebody-elses-state"})),
    );
    assert_eq!(status, 409, "{body}");
    let (_, _, message) = refusal(&body);
    assert!(
        message.contains("not the one this sign-in started with"),
        "{message}"
    );
    let (status, body) = gateway.console(
        &complete,
        Method::POST,
        Some(&json!({"code": "some-code#somebody-elses-state"})),
    );
    assert_eq!(status, 404, "{body}");
    let (_, _, message) = refusal(&body);
    assert!(message.contains("begin one again"), "{message}");
}

#[test]
fn an_id_nobody_began_is_refused() {
    let gateway = Gateway::start("manual-sign-in-unknown-id", VAULT);
    let (status, body) = gateway.console(
        &format!("{BEGIN}/never-begun"),
        Method::POST,
        Some(&json!({"code": "code#state"})),
    );
    assert_eq!(status, 404, "{body}");
}
