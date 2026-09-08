//! Subscription plan usage, driven as the audiences that ask for it.
//!
//! One capability replaced four per-audience refreshes, and the claim worth
//! testing is that every audience is answered from one reading of one ledger
//! and one inventory, narrowed by what it proved. These stories drive the real
//! released binary over real HTTP against a real Skarbiec vault this test
//! created and seeded with real `skarbiec` writes.
//!
//! Two things this capability does are not testable on this machine and are
//! not faked here:
//!
//! - The account holder's audience proves a session against the production
//!   Wisent Identity project. The in-test HTTP authority that used to answer
//!   in its place is deleted, and so is that story.
//! - Reading a plan's usage from the provider's own report needs a real
//!   provider subscription grant -- what `tests/providers/*_real.rs` needs.
//!   The in-test endpoint that used to answer as Anthropic and Kimi is
//!   deleted, and so are the two stories that asserted a reading and a
//!   provider refusal. What survives here is everything that does not require
//!   the vendor: the narrowing, the refusals, and the ledger-against-inventory
//!   disagreement.

#[path = "../support/gateway.rs"]
mod gateway;
#[path = "../support/mod.rs"]
mod support;

use reqwest::Method;
use serde_json::{json, Value};

use gateway::{
    answered_ids, assert_pool_answered, refusal, Gateway, AGENT, AGENT_SIGNING_SECRET, OTHER_AGENT,
    PLAN_USAGE, POOL, STRANGER_BEARER,
};
use support::SkarbiecVault;

/// Three accounts for the agent and one owned by another agent, which is what
/// proves a scoped answer is narrowed rather than merely filtered.
const CLAUDE: &str = "usage-agent-claude";
const KIMI: &str = "usage-agent-kimi";
const OPENAI: &str = "usage-agent-openai";
const OTHER: &str = "usage-other-claude";
const VAULT: &[(&str, &str, &str)] = &[
    (AGENT, "claude-code", CLAUDE),
    (AGENT, "kimi", KIMI),
    (AGENT, "openai", OPENAI),
    (OTHER_AGENT, "claude-code", OTHER),
];

fn row<'a>(report: &'a Value, id: &str) -> &'a Value {
    report["subscriptions"]
        .as_array()
        .expect("plan usage answers a subscriptions array")
        .iter()
        .find(|row| row["id"] == id)
        .unwrap_or_else(|| panic!("{id} is absent from {report}"))
}

/// The failure envelope recorded against one account, as a caller reads it.
fn error_for<'a>(report: &'a Value, id: &str) -> &'a Value {
    report["errors"]
        .as_array()
        .expect("plan usage answers an errors array")
        .iter()
        .find(|error| {
            error
                .pointer("/context/subscription")
                .and_then(Value::as_str)
                == Some(id)
        })
        .unwrap_or_else(|| panic!("no failure names {id} in {report}"))
}

/// The console proves this installation, so it is answered about every plan
/// the deployment holds, including the one owned by an agent that never
/// called.
#[test]
fn plan_usage_answers_the_console_about_every_plan_the_deployment_holds() {
    let gateway = Gateway::start("usage-console", VAULT);
    let (status, report) = gateway.console(PLAN_USAGE, Method::POST, None);
    assert_eq!(status, 200, "{report}");
    assert_pool_answered(&report);
    assert_eq!(report["scope"], "deployment");
    let mut answered = answered_ids(&report);
    answered.sort();
    assert_eq!(answered, vec![CLAUDE, KIMI, OPENAI, OTHER]);
}

/// The signed agent proves an agent, and the same capability answers it about
/// its own plans only. Its bearer is model-scoped, which is the shape every
/// workload identity has, and the four routes this replaced refused that shape
/// outright.
#[test]
fn plan_usage_answers_a_signed_agent_only_about_its_own_plans() {
    let gateway = Gateway::start("usage-agent", VAULT);
    let (status, report) = gateway.agent(PLAN_USAGE, Method::POST, None);
    assert_eq!(status, 200, "{report}");
    assert_pool_answered(&report);
    assert_eq!(report["scope"], AGENT);
    let mut answered = answered_ids(&report);
    answered.sort();
    assert_eq!(answered, vec![CLAUDE, KIMI, OPENAI]);
    assert!(
        !answered.contains(&OTHER.to_owned()),
        "an agent must not be told about another agent's plan: {report}"
    );
}

/// An account with no reading yet is reported as exactly that. A missing
/// measurement is never reported as zero usage, which is why the run is
/// incomplete rather than green.
#[test]
fn plan_usage_never_reports_an_unread_plan_as_unused() {
    let gateway = Gateway::start("usage-unread", VAULT);
    let (status, report) = gateway.agent(PLAN_USAGE, Method::POST, None);
    assert_eq!(status, 200, "{report}");
    assert_eq!(report["ok"], false, "{report}");
    let unread = row(&report, CLAUDE);
    assert!(
        unread["usage_source"].is_null(),
        "an unread plan must name no source: {unread}"
    );
    assert!(
        unread["limits"]
            .as_array()
            .expect("the plan windows")
            .is_empty(),
        "an unread plan must publish no window: {unread}"
    );
}

/// A caller that proved nothing is told nothing, and a bearer that proves only
/// transport is not an audience.
#[test]
fn plan_usage_refuses_a_caller_that_proved_no_identity() {
    let gateway = Gateway::start("usage-unproven", VAULT);
    let (status, body) = gateway.request(PLAN_USAGE, Method::POST, None, None, None);
    assert_eq!(status, 401, "{body}");
    assert_eq!(
        refusal(&body),
        (
            "unauthenticated".to_owned(),
            "authentication_error".to_owned(),
            "unauthorized".to_owned(),
        ),
    );

    let (status, body) =
        gateway.request(PLAN_USAGE, Method::POST, Some(STRANGER_BEARER), None, None);
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

/// Two proofs that contradict each other are never resolved in favour of
/// either, and a caller has nothing to say here: the answer follows from the
/// identity, so a body is refused rather than signed over and ignored.
#[test]
fn plan_usage_refuses_contradicting_proofs_and_a_request_that_asks_for_something() {
    let gateway = Gateway::start("usage-wrong-owner", VAULT);
    let (status, body) = gateway.request(
        PLAN_USAGE,
        Method::POST,
        Some(gateway::AGENT_BEARER),
        None,
        Some((OTHER_AGENT, AGENT_SIGNING_SECRET)),
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

    let (status, body) = gateway.console(
        PLAN_USAGE,
        Method::POST,
        Some(&json!({"agent_id": OTHER_AGENT})),
    );
    assert_eq!(status, 400, "{body}");
    assert_eq!(
        refusal(&body),
        (
            "invalid_request".to_owned(),
            "request_error".to_owned(),
            "plan usage takes no request fields; the answer follows from the proven identity"
                .to_owned(),
        ),
    );
}

/// An account the ledger remembers and the vault no longer holds is reported
/// as exactly that. Its grant cannot be confirmed, so its usage is not
/// restated as current and the run says why, in Skarbiec's name.
///
/// The vault stops holding it the way it really stops holding one: a real
/// `skarbiec delete` of the real item, mid-story.
#[test]
fn plan_usage_reports_an_account_the_vault_no_longer_holds() {
    let gateway = Gateway::start("usage-forgotten", VAULT);
    let (status, banked) = gateway.agent(
        POOL,
        Method::POST,
        Some(&json!({
            "action": "bank",
            "provider": "claude-code",
            "subscription_id": CLAUDE,
            "api_key": "usage-capability-opaque-grant",
        })),
    );
    assert_eq!(status, 200, "{banked}");
    let (status, first) = gateway.console(PLAN_USAGE, Method::POST, None);
    assert_eq!(status, 200, "{first}");
    assert_eq!(row(&first, CLAUDE)["status"], "active", "{first}");

    gateway
        .vault()
        .delete_item(&SkarbiecVault::item_id("claude-code", CLAUDE));

    let (status, second) = gateway.console(PLAN_USAGE, Method::POST, None);
    assert_eq!(status, 200, "{second}");
    assert_eq!(second["ok"], false, "{second}");
    let forgotten = row(&second, CLAUDE);
    assert_eq!(forgotten["status"], "undiscovered", "{forgotten}");
    assert_eq!(forgotten["state"], "unknown", "{forgotten}");
    assert_eq!(
        error_for(&second, CLAUDE)["detail"],
        "subscription exists in usage history but was not returned by Skarbiec; its current \
         credential cannot be confirmed",
    );
}
