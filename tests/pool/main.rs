//! The subscription pool, driven as the three audiences that ask it.
//!
//! One capability replaced eleven invocations, and the claim that makes the
//! replacement worth making is that the three audiences get one answer
//! narrowed by who they are. That is what these stories drive: the real
//! released binary over real HTTP, one isolated vault holding two agents'
//! accounts, and each audience presenting exactly the proof it has. A story
//! asserts what the pool answered and what the gateway persisted, with the
//! refusal sentences quoted as a caller receives them.
//!
//! ```console
//! $ cargo test --release --test pool -- --nocapture
//! ```

#[path = "authority.rs"]
mod authority;
#[path = "harness.rs"]
mod harness;
#[path = "../support/mod.rs"]
mod support;

use reqwest::Method;
use serde_json::json;

use authority::account_agent_id;
use harness::{
    answered_ids, assert_pool_answered, refusal, Gateway, AGENT, AGENT_SIGNING_SECRET, OTHER_AGENT,
    STRANGER_BEARER,
};
use support::vault_item;

/// The accounts the isolated vault holds: two for the agent, one for another
/// agent it must never be told about, one for the account holder's own user.
fn vault() -> Vec<serde_json::Value> {
    vec![
        vault_item(AGENT, "openai", "pool-agent-openai"),
        vault_item(AGENT, "anthropic", "pool-agent-anthropic"),
        vault_item(OTHER_AGENT, "openai", "pool-other-openai"),
        vault_item(&account_agent_id(), "openai", "pool-account-openai"),
    ]
}

/// Every row the pool answers carries the same fields, whoever asked.
fn assert_pool_row_shape(report: &serde_json::Value, audience: &str) {
    for row in report["subscriptions"]
        .as_array()
        .expect("the pool answers a subscriptions array")
    {
        for field in [
            "id",
            "provider",
            "status",
            "state",
            "expires_at",
            "last_redeem_error",
            "limits",
            "credential",
            "usage_check",
            "stale",
        ] {
            assert!(
                row.get(field).is_some(),
                "{audience} received a row without `{field}`: {row}"
            );
        }
    }
}

/// The console proves this installation, so it is answered about every account
/// the deployment holds -- including the one owned by an agent that never
/// called, which is the whole point of a deployment-scoped read.
#[test]
fn the_pool_answers_the_console_about_every_account_the_deployment_holds() {
    let gateway = Gateway::start("pool-console", &vault());
    let (status, report) = gateway.console(Method::GET, None);
    assert_eq!(status, 200, "{report}");
    assert_pool_answered(&report);
    assert_eq!(report["scope"], "deployment");
    let mut answered = answered_ids(&report);
    answered.sort();
    assert_eq!(
        answered,
        vec![
            "pool-account-openai",
            "pool-agent-anthropic",
            "pool-agent-openai",
            "pool-other-openai",
        ],
    );
    assert_pool_row_shape(&report, "the console");
}

/// The signed agent proves an agent, so the same read is narrowed to the
/// accounts that agent owns. The other agent's account is absent, not marked
/// unavailable: an agent is not told what it may not spend.
#[test]
fn the_pool_answers_a_signed_agent_only_about_its_own_accounts() {
    let gateway = Gateway::start("pool-agent", &vault());
    let (status, report) = gateway.agent(Method::GET, None);
    assert_eq!(status, 200, "{report}");
    assert_pool_answered(&report);
    assert_eq!(report["scope"], AGENT);
    let mut answered = answered_ids(&report);
    answered.sort();
    assert_eq!(answered, vec!["pool-agent-anthropic", "pool-agent-openai"]);
    assert_pool_row_shape(&report, "a signed agent");
}

/// The account holder proves a Wisent session, and the owner is derived from
/// that verified session rather than from anything the request carried.
#[test]
fn the_pool_answers_an_account_holder_only_about_the_session_it_proved() {
    let gateway = Gateway::start("pool-account", &vault());
    let (status, report) = gateway.account(Method::GET, None);
    assert_eq!(status, 200, "{report}");
    assert_pool_answered(&report);
    assert_eq!(report["scope"], account_agent_id());
    assert_eq!(answered_ids(&report), vec!["pool-account-openai"]);
    assert_pool_row_shape(&report, "an account holder");
}

/// The write surface banks onto the owner the caller proved and retires out of
/// it, and both are visible in the pool and on disk afterwards.
#[test]
fn banking_and_retiring_through_the_pool_records_the_proven_owner() {
    let gateway = Gateway::start("pool-write", &vault());
    let (status, banked) = gateway.agent(
        Method::POST,
        Some(&json!({
            "action": "bank",
            "provider": "openai",
            "label": "banked by the agent itself",
            "api_key": "pool-capability-opaque-provider-key",
        })),
    );
    assert_eq!(status, 200, "{banked}");
    assert_eq!(banked["subscription"]["agent_id"], AGENT);
    assert_eq!(banked["subscription"]["status"], "active");
    assert!(
        banked["subscription"]["api_key"].is_null(),
        "the credential value must never come back: {banked}"
    );
    let banked_id = banked["subscription"]["id"]
        .as_str()
        .expect("the banked subscription is named")
        .to_owned();

    let (status, mine) = gateway.agent(Method::GET, None);
    assert_eq!(status, 200, "{mine}");
    assert!(
        answered_ids(&mine).contains(&banked_id),
        "the banked account is missing from its owner's pool: {mine}"
    );

    let (status, retired) = gateway.agent(
        Method::POST,
        Some(&json!({"action": "retire", "subscription_id": banked_id})),
    );
    assert_eq!(status, 200, "{retired}");
    assert_eq!(retired["ok"], true);

    let journal = std::fs::read_to_string(gateway.state_dir().join("journal.jsonl"))
        .expect("the gateway journaled the retirement");
    assert!(
        journal.lines().any(|line| {
            let record: serde_json::Value = serde_json::from_str(line).expect("journal record");
            record["kind"] == "retire" && record["id"] == banked_id.as_str()
        }),
        "no retirement record for {banked_id} in {journal}"
    );
    let (status, after) = gateway.agent(Method::GET, None);
    assert_eq!(status, 200, "{after}");
    assert!(
        !answered_ids(&after).contains(&banked_id),
        "a retired account is still in the pool: {after}"
    );
}

/// A caller that proved nothing is told nothing, and the sentence it is told
/// is the one the gateway's own envelope carries.
#[test]
fn the_pool_refuses_a_caller_that_proved_no_identity() {
    let gateway = Gateway::start("pool-unproven", &vault());
    let (status, body) = gateway.request(Method::GET, None, None, None, false);
    assert_eq!(status, 401, "{body}");
    assert_eq!(
        refusal(&body),
        (
            "unauthenticated".to_owned(),
            "authentication_error".to_owned(),
            "unauthorized".to_owned(),
        ),
    );

    let (status, body) = gateway.request(Method::GET, Some(STRANGER_BEARER), None, None, false);
    assert_eq!(status, 403, "{body}");
    assert_eq!(
        refusal(&body),
        (
            "forbidden".to_owned(),
            "authorization_error".to_owned(),
            "forbidden".to_owned(),
        ),
    );
}

/// Ownership is read off the proof. An agent naming an owner is refused
/// rather than obeyed, and an account it does not own is not found for it even
/// though the deployment holds it.
#[test]
fn the_pool_refuses_a_caller_that_names_an_owner_it_did_not_prove() {
    let gateway = Gateway::start("pool-wrong-owner", &vault());
    let (status, body) = gateway.agent(
        Method::POST,
        Some(&json!({
            "action": "retire",
            "agent_id": OTHER_AGENT,
            "subscription_id": "pool-other-openai",
        })),
    );
    assert_eq!(status, 400, "{body}");
    assert_eq!(
        refusal(&body),
        (
            "invalid_request".to_owned(),
            "request_error".to_owned(),
            "agent_id is derived from the proven identity and must not be sent".to_owned(),
        ),
    );

    let (status, body) = gateway.agent(
        Method::POST,
        Some(&json!({"action": "retire", "subscription_id": "pool-other-openai"})),
    );
    assert_eq!(status, 404, "{body}");
    assert_eq!(
        refusal(&body),
        (
            "subscription_not_found".to_owned(),
            "state_error".to_owned(),
            "subscription not found".to_owned(),
        ),
    );

    // A signature by another agent than the bearer is bound to contradicts
    // itself, and a contradiction between two proofs is never resolved in
    // favour of either.
    let (status, body) = gateway.request(
        Method::GET,
        Some(harness::AGENT_BEARER),
        None,
        Some((OTHER_AGENT, AGENT_SIGNING_SECRET)),
        false,
    );
    assert_eq!(status, 403, "{body}");
    assert_eq!(
        refusal(&body),
        (
            "forbidden".to_owned(),
            "authorization_error".to_owned(),
            "forbidden".to_owned(),
        ),
    );
}

/// A subscription nothing owns cannot be retired, and a write that names no
/// recognised action is refused before anything is written.
#[test]
fn the_pool_refuses_an_unknown_subscription_and_an_unknown_action() {
    let gateway = Gateway::start("pool-unknown", &vault());
    let (status, body) = gateway.agent(
        Method::POST,
        Some(&json!({"action": "retire", "subscription_id": "pool-nothing-owns-this"})),
    );
    assert_eq!(status, 404, "{body}");
    assert_eq!(
        refusal(&body),
        (
            "subscription_not_found".to_owned(),
            "state_error".to_owned(),
            "subscription not found".to_owned(),
        ),
    );

    let (status, body) = gateway.agent(Method::POST, Some(&json!({"action": "borrow"})));
    assert_eq!(status, 400, "{body}");
    assert_eq!(
        refusal(&body).2,
        "action must be \"bank\" or \"retire\"".to_owned(),
    );

    // The console is the only caller that may name an owner, and it must.
    let (status, body) = gateway.console(Method::POST, Some(&json!({"action": "retire"})));
    assert_eq!(status, 400, "{body}");
    assert_eq!(
        refusal(&body).2,
        "agent_id names the agent whose pool is written and is required for a \
         deployment-scoped write"
            .to_owned(),
    );

    assert!(
        !gateway.donated_file().exists(),
        "a refused write must not create the donated-subscription overlay"
    );
}
