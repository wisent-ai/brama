//! The accounts these stories are about, and the two ways they are reached.
//!
//! Kept beside the stories rather than inside them: what the vault holds and
//! which provider answers for it is the setting every story shares, and a
//! story that restates it is a story that can disagree with its neighbour
//! about what was being measured.

use reqwest::Method;
use serde_json::{json, Value};

use crate::authority::account_agent_id;
use crate::gateway::{Gateway, AGENT, OTHER_AGENT, POOL};
use crate::reports::ProviderReports;
use crate::support::vault_item;

/// The account whose provider answers its usage report, the account whose
/// provider refuses it, an account whose provider publishes no free report at
/// all, and one account owned by another agent.
pub const READABLE: &str = "usage-agent-claude";
pub const REFUSING: &str = "usage-agent-kimi";
pub const UNPUBLISHED: &str = "usage-agent-openai";
pub const OTHER: &str = "usage-other-claude";
/// The grant the agent banks through the pool, so a reading is taken with a
/// credential the product itself stored rather than one poked into place.
pub const GRANT: &str = "usage-capability-opaque-grant";

/// The account holder's own subscription, named after the user id the isolated
/// identity authority proves.
pub fn account() -> String {
    format!("usage-account-{}", account_agent_id())
}

pub fn vault() -> Vec<Value> {
    vec![
        vault_item(AGENT, "claude-code", READABLE),
        vault_item(AGENT, "kimi", REFUSING),
        vault_item(AGENT, "openai", UNPUBLISHED),
        vault_item(OTHER_AGENT, "claude-code", OTHER),
        vault_item(&account_agent_id(), "claude-code", &account()),
    ]
}

/// Point both publishing providers at the endpoint the story answers. The
/// override is the deployment's own, so the reader under test is unchanged.
pub fn provider_reports(reports: &ProviderReports) -> Vec<(&'static str, String)> {
    vec![
        ("BRAMA_PROVIDER_CLAUDE_CODE_BASE_URL", reports.origin()),
        ("BRAMA_PROVIDER_KIMI_BASE_URL", reports.origin()),
    ]
}

/// Bank the agent's own grants onto the accounts the routes table already
/// names, through the pool capability, as their owner.
pub fn bank_the_agents_grant(gateway: &Gateway) {
    for (provider, subscription) in [("claude-code", READABLE), ("kimi", REFUSING)] {
        let (status, banked) = gateway.agent(
            POOL,
            Method::POST,
            Some(&json!({
                "action": "bank",
                "provider": provider,
                "subscription_id": subscription,
                "api_key": GRANT,
            })),
        );
        assert_eq!(status, 200, "banking {subscription}: {banked}");
    }
}

pub fn row<'a>(report: &'a Value, id: &str) -> &'a Value {
    report["subscriptions"]
        .as_array()
        .expect("plan usage answers a subscriptions array")
        .iter()
        .find(|row| row["id"] == id)
        .unwrap_or_else(|| panic!("{id} is absent from {report}"))
}

/// The failure envelope recorded against one account, as a caller reads it.
pub fn error_for<'a>(report: &'a Value, id: &str) -> &'a Value {
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
