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
pub fn configured_request_sign_agents() -> Result<Vec<String>, String> {
    let mut agents: Vec<String> = capability_map(REQUEST_SIGN_CAPABILITIES_ENV)?
        .into_keys()
        .collect();
    let identities = capability_map(REQUEST_SIGN_IDENTITIES_ENV)?;
    for agent in CENTRAL_REQUEST_SIGN_AGENTS {
        if identities.contains_key(*agent) {
            agents.push((*agent).to_string());
        }
    }
    agents.sort();
    agents.dedup();
    Ok(agents)
}

/// Resolve an agent-specific request-signing secret immediately before HMAC
/// verification. Echo, legacy Content Platform, Oko, and Weles are strict
/// central-item projections; they never substitute generated agent resources
/// or another product.
pub async fn get_agent_auth_secret(agent_id: &str) -> Result<Secret, String> {
    if CENTRAL_REQUEST_SIGN_AGENTS.contains(&agent_id) {
        let secret = capability_map(REQUEST_SIGN_IDENTITIES_ENV)?.remove(agent_id)
            .ok_or_else(|| format!("request-sign identity `{agent_id}` is absent from {REQUEST_SIGN_IDENTITIES_ENV}"))?;
        if secret.is_empty() {
            return Err(format!(
                "request-sign identity `{agent_id}` in {REQUEST_SIGN_IDENTITIES_ENV} is empty"
            ));
        }
        return Ok(Secret::from_bytes(secret.into_bytes()));
    }

    let resource = format!("agent:{}", slug(agent_id));
    let seed_refusal = if let Some(id) =
        configured_capability(REQUEST_SIGN_CAPABILITIES_ENV, agent_id)?
    {
        let binding = CapabilityRef::request_sign(&id, &resource).map_err(|error| {
            format!("validate configured request-sign binding for `{resource}`: {error}")
        })?;
        let broker = client()
            .map_err(|error| format!("construct request-sign client for `{resource}`: {error}"))?;
        match broker.redeem(&binding) {
            Ok(secret) => return Ok(secret),
            Err(error) => Some(format!(
                "redeem configured request-sign capability for `{resource}`: {error}"
            )),
        }
    } else {
        None
    };
    // A seed can be spent or expired. Preserve its refusal while asking the
    // existing exact-resource authority paths for a fresh credential.
    let result = async {
        let fresh = match issue_capability(REQUEST_SIGN_PURPOSE, &resource).await {
            Ok(fresh) => fresh,
            Err(refused) => {
                return credential_by_grant(&resource)
                    .await
                    .map_err(|grant| grant.caused_by(refused).render());
            }
        };
        let binding = CapabilityRef::request_sign(&fresh, &resource).map_err(|error| {
            format!("validate issued request-sign binding for `{resource}`: {error}")
        })?;
        let broker = client()
            .map_err(|error| format!("construct request-sign client for `{resource}`: {error}"))?;
        match broker.redeem(&binding) {
            Ok(secret) => Ok(secret),
            Err(error) => credential_by_grant(&resource).await.map_err(|refused| {
                format!(
                    "{}; redeem issued request-sign capability for `{resource}`: {error}",
                    refused.render()
                )
            }),
        }
    }
    .await;
    result.map_err(|error| {
        let detail = match seed_refusal {
            Some(seed) => format!("{error}; prior refusal: {seed}"),
            None => error,
        };
        warn!(event = "request_sign_credential_unavailable", agent_id, %resource, %detail);
        format!("resolve request-sign secret for `{agent_id}` at `{resource}`: {detail}")
    })
}
