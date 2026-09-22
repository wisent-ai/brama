//! The subscription pool, driven as the audiences that ask it.
//!
//! One capability replaced eleven invocations, and the claim that makes the
//! replacement worth making is that each audience gets one answer narrowed by
//! who they are. That is what these stories drive: the real released binary
//! over real HTTP, a real Skarbiec vault this test created and seeded with
//! real `skarbiec` writes, and each audience presenting exactly the proof it
//! has. A story asserts what the pool answered and what the gateway
//! persisted, with the refusal sentences quoted as a caller receives them.
//!
//! The account holder is missing on purpose: that audience proves a session
//! against the production Wisent Identity project, and the in-test HTTP
//! authority that used to answer in its place has been deleted.

#[path = "../support/gateway.rs"]
mod gateway;
mod membership;
#[path = "../support/mod.rs"]
mod support;

use reqwest::Method;
use serde_json::json;

use gateway::{
    answered_ids, assert_pool_answered, refusal, Gateway, AGENT, AGENT_SIGNING_SECRET, OTHER_AGENT,
    POOL, STRANGER_BEARER,
};

/// The accounts the vault holds: two for the agent, one for another agent it
/// must never be told about.
pub(crate) const VAULT: &[(&str, &str, &str)] = &[
    (AGENT, "openai", "pool-agent-openai"),
    (AGENT, "anthropic", "pool-agent-anthropic"),
    (OTHER_AGENT, "openai", "pool-other-openai"),
];

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
    let gateway = Gateway::start("pool-console", VAULT);
    let (status, report) = gateway.console(POOL, Method::GET, None);
    assert_eq!(status, 200, "{report}");
    assert_pool_answered(&report);
    assert_eq!(report["scope"], "deployment");
    let mut answered = answered_ids(&report);
    answered.sort();
    assert_eq!(
        answered,
        vec![
            "pool-agent-anthropic",
            "pool-agent-openai",
            "pool-other-openai",
        ],
    );
    assert_pool_row_shape(&report, "the console");
}

/// The signed agent proves an agent and is answered about what it may spend.
/// Since 2026-09-16 that is every account the deployment holds - the other
/// agent's account included - because a subscription in the vault is in the
/// rotation for every caller; only the scope in the report says who asked.
#[test]
fn the_pool_answers_a_signed_agent_about_every_account_it_may_spend() {
    let gateway = Gateway::start("pool-agent", VAULT);
    let (status, report) = gateway.agent(POOL, Method::GET, None);
    assert_eq!(status, 200, "{report}");
    assert_pool_answered(&report);
    assert_eq!(report["scope"], AGENT);
    let mut answered = answered_ids(&report);
    answered.sort();
    assert_eq!(
        answered,
        vec![
            "pool-agent-anthropic",
            "pool-agent-openai",
            "pool-other-openai"
        ],
    );
    assert_pool_row_shape(&report, "a signed agent");
}

/// The write surface banks onto the owner the caller proved and retires out of
/// it, and both are visible in the pool and on disk afterwards.
#[test]
fn banking_and_retiring_through_the_pool_records_the_proven_owner() {
    let gateway = Gateway::start("pool-write", VAULT);
    let (status, banked) = gateway.agent(
        POOL,
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

    let (status, mine) = gateway.agent(POOL, Method::GET, None);
    assert_eq!(status, 200, "{mine}");
    assert!(
        answered_ids(&mine).contains(&banked_id),
        "the banked account is missing from its owner's pool: {mine}"
    );

    let (status, retired) = gateway.agent(
        POOL,
        Method::POST,
        Some(&json!({"action": "retire", "subscription_id": banked_id})),
    );
    assert_eq!(status, 200, "{retired}");
    assert_eq!(retired["ok"], true);

    let journal = std::fs::read_to_string(gateway.root().join("state").join("journal.jsonl"))
        .expect("the gateway journaled the retirement");
    assert!(
        journal.lines().any(|line| {
            let record: serde_json::Value = serde_json::from_str(line).expect("journal record");
            record["kind"] == "retire" && record["id"] == banked_id.as_str()
        }),
        "no retirement record for {banked_id} in {journal}"
    );
    let (status, after) = gateway.agent(POOL, Method::GET, None);
    assert_eq!(status, 200, "{after}");
    assert!(
        !answered_ids(&after).contains(&banked_id),
        "a retired account is still in the pool: {after}"
    );
}


#[path = "http/refusals.rs"]
mod refusals;


/// How many accounts the pool holds is not how many members it has, and the
/// document says which is which.
///
/// A deployment holding five provider accounts read as fifteen because every
/// member counted as one: the slots left behind by earlier imports, and the
/// ids the usage ledger still remembers after the vault stopped listing them,
/// are indistinguishable from an account in a bare row count. Two members
/// recording one account are one account; a member recording none is not an
/// account at all, however exactly its id or its login row names one -- one
/// login row signs both the Claude Code and the Codex account of one person
/// in, so a login row cannot stand for an account.
#[test]
fn the_pool_counts_accounts_and_names_what_it_cannot_attribute_to_one() {
    let gateway = Gateway::start("pool-accounts", &[]);
    gateway.vault().seed_account_subscription(
        "openai",
        "pool-primary",
        "pool-account@example.invalid",
        "pool-account-login",
    );
    gateway.vault().seed_account_subscription(
        "openai",
        "pool-secondary",
        "pool-account@example.invalid",
        "pool-account-login",
    );
    gateway
        .vault()
        .seed_login_only_subscription("openai", "pool-login-only", "pool-account-login");
    gateway
        .vault()
        .seed_marked_subscription("openai", "pool-slot");

    let (status, report) = gateway.console(POOL, Method::GET, None);
    assert_eq!(status, 200, "{report}");
    let accounts = &report["accounts"];
    assert_eq!(accounts["total"], 1, "{report}");
    assert_eq!(
        accounts["per_provider"]["openai"]
            .as_array()
            .map(Vec::as_slice),
        Some([serde_json::json!("pool-account@example.invalid")].as_slice()),
        "two members recording one account are one account, named: {report}"
    );
    assert_eq!(
        accounts["members_without_account"]
            .as_array()
            .map(|members| {
                members
                    .iter()
                    .filter_map(serde_json::Value::as_str)
                    .collect::<Vec<_>>()
            }),
        Some(vec!["pool-login-only", "pool-slot"]),
        "a member that records no account is named and not counted, and a login row is not an \
         account: {report}"
    );
    assert_eq!(
        report["subscriptions"].as_array().map(Vec::len),
        Some(4),
        "the members are still all answered: {report}"
    );
}
