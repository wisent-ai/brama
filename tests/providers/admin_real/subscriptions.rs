//! The agent subscription pool, banked and retired with a real account.
//!
//! Banking, probing, replacing, listing and retiring all run through the real
//! released binary; the retired account is observed as the refusal the probe
//! answers with, not as an absence nobody reports.

use serde_json::json;

use super::gateway::{real_provider_credential, Gateway};

const AGENT: &str = "brama-real-qualification";
const POOL: &str = "/v1/subscription-pool";

#[test]
fn banking_probing_and_retiring_a_pool_account_uses_a_real_openrouter_account() {
    let credential = real_provider_credential("openrouter");
    let gateway = Gateway::start();
    let (status, created) = gateway.admin(
        reqwest::Method::POST,
        POOL,
        Some(json!({
            "action": "bank",
            "agent_id": AGENT,
            "provider": "openrouter",
            "label": "primary",
            "api_key": credential,
        })),
    );
    assert_eq!(status, 200, "{created}");
    let id = created["subscription"]["id"]
        .as_str()
        .expect("subscription id")
        .to_owned();
    let probe = format!("/v1/admin/subscriptions/{AGENT}/{id}/probe");
    let (status, proved) = gateway.admin(reqwest::Method::POST, &probe, None);
    assert_eq!(status, 200, "{proved}");
    assert_eq!(proved["ok"], true);

    let credential = real_provider_credential("openrouter");
    let (status, replaced) = gateway.admin(
        reqwest::Method::POST,
        POOL,
        Some(json!({
            "action": "bank",
            "agent_id": AGENT,
            "provider": "openrouter",
            "label": "replacement",
            "api_key": credential,
        })),
    );
    assert_eq!(status, 200, "{replaced}");
    assert_eq!(replaced["subscription"]["id"], id);
    assert_eq!(replaced["subscription"]["label"], "replacement");
    let (status, proved) = gateway.admin(reqwest::Method::POST, &probe, None);
    assert_eq!(status, 200, "{proved}");

    let (status, pooled) = gateway.admin(reqwest::Method::GET, POOL, None);
    assert_eq!(status, 200, "{pooled}");
    assert_eq!(pooled["scope"], "deployment");
    assert!(
        pooled["subscriptions"]
            .as_array()
            .expect("pool rows")
            .iter()
            .any(|row| row["id"] == id.as_str()),
        "the banked account is missing from the pool: {pooled}"
    );

    let (status, retired) = gateway.admin(
        reqwest::Method::POST,
        POOL,
        Some(json!({"action": "retire", "agent_id": AGENT, "subscription_id": id})),
    );
    assert_eq!(status, 200, "{retired}");
    let (status, missing) = gateway.admin(reqwest::Method::POST, &probe, None);
    assert_eq!(status, 404, "{missing}");
}
