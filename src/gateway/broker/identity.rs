//! The agent identity this installation signs and verifies requests with.
//!
//! A request-signing secret is not a provider credential: it proves which
//! fleet agent is calling, it belongs to this deployment rather than to a
//! paid account, and the strict central agents are read straight from a
//! projected item instead of being issued at all. That is why it is its own
//! subject -- the table of who may be signed for, and the order in which one
//! agent's secret is looked for, change for reasons that have nothing to do
//! with any provider.

use tracing::warn;

use super::vault::{credential_by_grant, issue_capability, REQUEST_SIGN_PURPOSE};
use super::{capability_map, client, configured_capability, slug};
use crate::capability::{CapabilityRef, Secret};

const REQUEST_SIGN_CAPABILITIES_ENV: &str = "BRAMA_REQUEST_SIGN_CAPABILITY_IDS";
const REQUEST_SIGN_IDENTITIES_ENV: &str = "BRAMA_REQUEST_SIGN_IDENTITIES";
const CENTRAL_REQUEST_SIGN_AGENTS: &[&str] = &[
    "echo",
    "content-platform",
    "oko",
    "weles",
    "lem",
    "probierz",
    "wisent-app",
];

/// Every agent this installation is configured to sign for, so a readiness
/// check can ask what each of them could actually route. Readiness had no way
/// to name an agent, which is why it could only report the direct-API providers
/// and said nothing about the subscription-backed ones.
pub fn configured_request_sign_agents() -> Vec<String> {
    let mut agents: Vec<String> = capability_map(REQUEST_SIGN_CAPABILITIES_ENV)
        .map(|map| map.into_keys().collect())
        .unwrap_or_default();
    for agent in CENTRAL_REQUEST_SIGN_AGENTS {
        if capability_map(REQUEST_SIGN_IDENTITIES_ENV).is_some_and(|map| map.contains_key(*agent)) {
            agents.push((*agent).to_string());
        }
    }
    agents.sort();
    agents.dedup();
    agents
}

/// Resolve an agent-specific request-signing secret immediately before HMAC
/// verification. Echo, legacy Content Platform, Oko, and Weles are strict
/// central-item projections; they never substitute generated agent resources
/// or another product.
pub async fn get_agent_auth_secret(agent_id: &str) -> Option<Secret> {
    if CENTRAL_REQUEST_SIGN_AGENTS.contains(&agent_id) {
        let secret = capability_map(REQUEST_SIGN_IDENTITIES_ENV)?.remove(agent_id)?;
        return Some(Secret::from_bytes(secret.into_bytes()));
    }

    let resource = format!("agent:{}", slug(agent_id));
    if let Some(capability_id) = configured_capability(REQUEST_SIGN_CAPABILITIES_ENV, agent_id) {
        if let Ok(binding) = CapabilityRef::request_sign(&capability_id, &resource) {
            if let Ok(secret) = client()?.redeem(&binding) {
                return Some(secret);
            }
        }
    }
    // Same reasoning as a provider credential: an optional seed is short-lived
    // by contract, so its refusal is steady state and a fresh capability is the
    // answer, not an error. Managed launches do not pre-issue one.
    // The provider path already says when the authority refuses; this one
    // returned None in silence, and the caller sees only "no auth secret for
    // agent" -- a sentence that fits a missing item, a refused issue and a
    // denied redemption equally well.
    let fresh = match issue_capability(REQUEST_SIGN_PURPOSE, &resource).await {
        Ok(fresh) => fresh,
        Err(refused) => {
            warn!(
                event = "request_sign_issue_failed",
                agent_id,
                %resource,
                envelope = %refused.to_json(),
                "the authority would not issue a request-sign capability; trying the read grant"
            );
            return match credential_by_grant(&resource).await {
                Ok(secret) => Some(secret),
                Err(grant_refusal) => {
                    let refusal = grant_refusal.caused_by(refused);
                    warn!(
                        event = "request_sign_credential_unavailable",
                        agent_id,
                        envelope = %refusal.to_json(),
                        "{}",
                        refusal.render()
                    );
                    None
                }
            };
        }
    };
    let binding = match CapabilityRef::request_sign(&fresh, &resource) {
        Ok(binding) => binding,
        Err(error) => {
            warn!(
                event = "request_sign_binding_invalid",
                agent_id, %resource, %error,
                "the issued capability does not bind to this resource"
            );
            return None;
        }
    };
    match client()?.redeem(&binding) {
        Ok(secret) => Some(secret),
        Err(error) => {
            warn!(
                event = "request_sign_redeem_failed",
                agent_id, %resource, %error,
                "the authority issued a request-sign capability and refused to redeem it; \
                 trying the read grant"
            );
            match credential_by_grant(&resource).await {
                Ok(secret) => Some(secret),
                Err(refusal) => {
                    warn!(
                        event = "request_sign_credential_unavailable",
                        agent_id,
                        envelope = %refusal.to_json(),
                        "{}",
                        refusal.render()
                    );
                    None
                }
            }
        }
    }
}
