//! Capability-backed credential seams for Brama.
//!
//! Capability identifiers are trusted deployment configuration and remain
//! opaque. Plaintext is redeemed only at HMAC verification or provider
//! invocation boundaries; it is never materialized in a file or JSON.

use std::collections::HashMap;

use serde::Deserialize;

use crate::capability::{CapabilityClient, CapabilityRef, Secret};

const ENTITLEMENTS_ROUTER_BIN_ENV: &str = "ENTITLEMENTS_ROUTER_BIN";
const DEFAULT_ENTITLEMENTS_ROUTER_BIN: &str = "entitlements-router";
const REQUEST_SIGN_CAPABILITIES_ENV: &str = "BRAMA_REQUEST_SIGN_CAPABILITY_IDS";
const PROVIDER_CAPABILITIES_ENV: &str = "BRAMA_PROVIDER_CAPABILITY_IDS";

/// Fold an identifier into the stable resource alphabet used by deployment
/// bindings. The original identifier remains the lookup key in trusted config.
pub fn slug(value: &str) -> String {
    value
        .trim()
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

#[derive(Debug, Clone, Deserialize)]
pub struct SubscriptionEntry {
    pub id: String,
    pub provider: String,
    pub status: String,
}

#[derive(Debug, Deserialize)]
struct BrokerItems {
    #[serde(default)]
    items: Vec<BrokerSubscriptionEntry>,
}

#[derive(Debug, Deserialize)]
struct BrokerSubscriptionEntry {
    id: Option<String>,
    provider: Option<String>,
    agent_id: Option<String>,
    status: Option<String>,
}

fn capability_map(name: &str) -> Option<HashMap<String, String>> {
    let encoded = std::env::var(name).ok()?;
    let parsed: HashMap<String, String> = serde_json::from_str(&encoded).ok()?;
    if parsed.is_empty() {
        return None;
    }
    Some(parsed)
}

fn configured_capability(name: &str, key: &str) -> Option<String> {
    capability_map(name)?.remove(key)
}

fn client() -> Option<CapabilityClient> {
    CapabilityClient::from_env().ok()
}

/// Redeem an agent-specific request-signing secret immediately before HMAC
/// verification. The capability ID comes only from trusted process config.
pub async fn get_agent_auth_secret(agent_id: &str) -> Option<Secret> {
    let capability_id = configured_capability(REQUEST_SIGN_CAPABILITIES_ENV, agent_id)?;
    let resource = format!("agent:{}", slug(agent_id));
    let binding = CapabilityRef::request_sign(&capability_id, &resource).ok()?;
    client()?.redeem(&binding).ok()
}

/// Return whether trusted deployment config contains a locally valid direct
/// provider capability. This never redeems or handles plaintext.
pub fn provider_capability_configured(provider: &str) -> bool {
    let Some(capability_id) = configured_capability(PROVIDER_CAPABILITIES_ENV, provider) else {
        return false;
    };
    let resource = format!("provider:{}", slug(provider));
    CapabilityRef::provider(&capability_id, &resource).is_ok() && client().is_some()
}

/// Redeem a direct provider API credential immediately before the HTTP call.
pub async fn provider_credential(provider: &str) -> Option<Secret> {
    let capability_id = configured_capability(PROVIDER_CAPABILITIES_ENV, provider)?;
    let resource = format!("provider:{}", slug(provider));
    let binding = CapabilityRef::provider(&capability_id, &resource).ok()?;
    client()?.redeem(&binding).ok()
}

/// Redeem one subscription provider credential immediately before its CLI call.
/// The local resource binds both provider and subscription to prevent cross-use.
pub async fn subscription_credential(subscription_id: &str, provider: &str) -> Option<Secret> {
    let capability_id = configured_capability(PROVIDER_CAPABILITIES_ENV, subscription_id)?;
    let resource = format!("provider:{}:{}", slug(provider), slug(subscription_id));
    let binding = CapabilityRef::provider(&capability_id, &resource).ok()?;
    client()?.redeem(&binding).ok()
}

/// Enumerate one agent's subscription metadata through the entitlements broker.
/// Missing binaries, failed commands, malformed output, and incomplete rows fail
/// closed so dispatch never falls back to a database or an unbound subscription.
pub async fn list_subscriptions(agent_id: &str) -> Vec<SubscriptionEntry> {
    list_subscriptions_result(agent_id)
        .await
        .unwrap_or_default()
}

fn subscription_prefix(agent_id: &str) -> String {
    format!("brama-sub-{}-", slug(agent_id))
}

fn complete_field(value: Option<String>) -> Option<String> {
    value.filter(|field| !field.is_empty() && field.trim() == field)
}

fn parse_subscriptions(output: &[u8], agent_id: &str) -> Result<Vec<SubscriptionEntry>, ()> {
    let prefix = subscription_prefix(agent_id);
    let response: BrokerItems = serde_json::from_slice(output).map_err(|_| ())?;

    Ok(response
        .items
        .into_iter()
        .filter_map(|entry| {
            let id = complete_field(entry.id)?;
            let provider = complete_field(entry.provider)?;
            let entry_agent_id = complete_field(entry.agent_id)?;
            let status = complete_field(entry.status)?;
            if !id.starts_with(&prefix) || entry_agent_id != agent_id {
                return None;
            }
            Some(SubscriptionEntry {
                id,
                provider,
                status,
            })
        })
        .collect())
}

async fn list_subscriptions_result(agent_id: &str) -> Result<Vec<SubscriptionEntry>, ()> {
    let broker = std::env::var(ENTITLEMENTS_ROUTER_BIN_ENV)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_ENTITLEMENTS_ROUTER_BIN.to_owned());
    list_subscriptions_with_broker(&broker, agent_id).await
}

async fn list_subscriptions_with_broker(
    broker: &str,
    agent_id: &str,
) -> Result<Vec<SubscriptionEntry>, ()> {
    let output = tokio::process::Command::new(broker)
        .arg("list-items")
        .arg(subscription_prefix(agent_id))
        .output()
        .await
        .map_err(|_| ())?;
    if !output.status.success() {
        return Err(());
    }
    parse_subscriptions(&output.stdout, agent_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slug_is_stable_for_local_resource_binding() {
        assert_eq!(slug(" Claude_Code / Primary "), "claude-code---primary");
    }

    #[test]
    fn malformed_capability_maps_fail_closed() {
        let name = "BRAMA_TEST_INVALID_CAPABILITY_MAP";
        std::env::set_var(name, "not-json");
        assert!(capability_map(name).is_none());
        std::env::remove_var(name);
    }

    #[test]
    fn parses_subscription_items_envelope() {
        let output = br#"{
            "items": [
                {
                    "id": "brama-sub-agent-a-claude",
                    "provider": "claude-code",
                    "agent_id": "Agent A",
                    "status": "active"
                },
                {
                    "id": "brama-sub-agent-a-codex",
                    "provider": "codex",
                    "agent_id": "Agent A",
                    "status": "retired"
                }
            ],
            "count": 2
        }"#;

        let subscriptions = parse_subscriptions(output, "Agent A").expect("valid broker output");

        assert_eq!(subscriptions.len(), 2);
        assert_eq!(subscriptions[0].id, "brama-sub-agent-a-claude");
        assert_eq!(subscriptions[0].provider, "claude-code");
        assert_eq!(subscriptions[0].status, "active");
        assert_eq!(subscriptions[1].status, "retired");
    }

    #[test]
    fn drops_unbound_and_incomplete_subscription_items() {
        let output = br#"{
            "items": [
                {
                    "id": "brama-sub-agent-a-wrong-agent",
                    "provider": "claude-code",
                    "agent_id": "Agent B",
                    "status": "active"
                },
                {
                    "id": "brama-sub-agent-b-wrong-prefix",
                    "provider": "claude-code",
                    "agent_id": "Agent A",
                    "status": "active"
                },
                {
                    "id": "brama-sub-agent-a-missing-provider",
                    "provider": null,
                    "agent_id": "Agent A",
                    "status": "active"
                },
                {
                    "id": "brama-sub-agent-a-missing-status",
                    "provider": "codex",
                    "agent_id": "Agent A",
                    "status": null
                }
            ],
            "count": 4
        }"#;

        let subscriptions = parse_subscriptions(output, "Agent A").expect("valid broker output");

        assert!(subscriptions.is_empty());
    }

    #[tokio::test]
    async fn invokes_broker_with_bound_subscription_prefix() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir().expect("create broker fixture directory");
        let broker = directory.path().join("entitlements-router");
        std::fs::write(
            &broker,
            r#"#!/bin/sh
[ "$1" = "list-items" ] || exit 20
[ "$2" = "brama-sub-agent-a-" ] || exit 21
printf '%s\n' '{"items":[{"id":"brama-sub-agent-a-claude","provider":"claude-code","agent_id":"Agent A","status":"active"}],"count":1}'
"#,
        )
        .expect("write broker fixture");
        let mut permissions = std::fs::metadata(&broker)
            .expect("read broker fixture metadata")
            .permissions();
        permissions.set_mode(0o700);
        std::fs::set_permissions(&broker, permissions).expect("make broker fixture executable");

        let subscriptions =
            list_subscriptions_with_broker(broker.to_str().expect("UTF-8 fixture path"), "Agent A")
                .await
                .expect("broker command succeeds");

        assert_eq!(subscriptions.len(), 1);
        assert_eq!(subscriptions[0].id, "brama-sub-agent-a-claude");
    }
}
