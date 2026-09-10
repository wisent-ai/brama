//! Credential seams for Brama.
//!
//! Capability redemption through the local Skarbiec broker is authoritative
//! for managed installations. A standalone desktop installation may instead
//! install an in-memory provider credential map before the server starts.
//!
//! This file owns the redemption itself: the coordinate a credential is read
//! from, the order the two request paths are tried in, and the failure a
//! caller is told when neither produced one. Everything it needs to do that is
//! its own module, because each changes for its own reason -- `vault` is the
//! seam to Skarbiec, `binding` is what this deployment was configured to
//! authenticate, `identity` is the agent secret a signed request is verified
//! against, `standalone` is the desktop map that replaces the vault entirely,
//! and `subscription` is everything about a paid account behind one call.

mod binding;
mod identity;
mod standalone;
mod subscription;
mod vault;

use std::collections::HashMap;

use tracing::warn;

use crate::capability::{CapabilityClient, CapabilityRef, Secret};
use crate::core::failure::{self, POINT_CREDENTIAL_REDEEM};
use wisent_errors::{Code, Failure};

use binding::PROVIDER_CAPABILITIES_ENV;
use standalone::{local_provider_credential, LOCAL_SUBSCRIPTION_CREDENTIALS};
use subscription::refresh_subscription_credential_inner;
use vault::{credential_by_grant, issue_capability, PROVIDER_PURPOSE};

// The surface the rest of the crate calls, named one by one so every existing
// path -- in the gateway, in the dispatcher, in the CLI and in the tests --
// resolves to exactly what it resolved to before, and nothing else comes with
// it.
pub use binding::{configured_provider_capabilities, provider_capability_configured};
pub use identity::{configured_request_sign_agents, get_agent_auth_secret};
pub use standalone::{
    install_local_provider_credentials, local_provider_credentials_enabled, local_provider_names,
    put_local_provider_credential, remove_donated_credential, remove_local_provider_credential,
};
pub use subscription::{
    discover_subscriptions, donated_add, donated_remove, donated_subscriptions_path,
    list_all_subscriptions, list_recoverable_subscriptions, list_subscriptions,
    list_unroutable_accounts, put_donated_credential, refresh_subscription_credential,
    refresh_subscription_credential_ahead, subscription_tags_for_write, supports_oauth_refresh,
    DonationRefusal, RefreshAhead, SubscriptionEntry, UnroutableAccount,
};

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

/// The coordinate a direct provider credential is read from.
///
/// The dispatch path names it in provider failures, because the repair to a
/// credential that cannot be used is always at the coordinate it came from,
/// and a message that omits it sends the reader looking for the item first.
pub fn provider_resource(provider: &str) -> String {
    format!("provider:{}", slug(provider))
}

/// The coordinate one subscription's credential is read from.
pub fn subscription_resource(provider: &str, subscription_id: &str) -> String {
    format!("provider:{}:{}", slug(provider), slug(subscription_id))
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

/// Redeem a direct provider API credential immediately before the HTTP call.
///
/// Managed launches issue at final use because a short-lived, single-use id
/// cannot be refreshed through a process environment. An optional seed is
/// accepted for standalone callers, but its refusal is steady state: obtain a
/// fresh capability and redeem that. Neither path holds plaintext beyond the
/// returned [`Secret`].
pub async fn provider_credential(provider: &str) -> Option<Secret> {
    if !crate::providers::adapter::provider_requires_credential(provider) {
        return Some(Secret::from_bytes(Vec::new()));
    }
    if local_provider_credentials_enabled() {
        return local_provider_credential(provider);
    }
    let resource = provider_resource(provider);
    let mut prior = None;
    if let Some(capability_id) = configured_capability(PROVIDER_CAPABILITIES_ENV, provider) {
        match redeem_provider_resource(&capability_id, &resource).await {
            Ok(secret) => return Some(secret),
            Err(refused) => prior = Some(refused),
        }
    }
    match issue_capability(PROVIDER_PURPOSE, &resource).await {
        Ok(fresh) => match redeem_provider_resource(&fresh, &resource).await {
            Ok(secret) => return Some(secret),
            Err(refused) => prior = Some(append_failure_cause(refused, prior)),
        },
        Err(refused) => prior = Some(append_failure_cause(refused, prior)),
    }
    match credential_by_grant(&resource).await {
        Ok(secret) => Some(secret),
        Err(refused) => {
            let refused = append_failure_cause(refused, prior).with_context("provider", provider);
            warn!(
                event = "provider_credential_unavailable",
                provider,
                envelope = %refused.to_json(),
                "{}",
                refused.render()
            );
            None
        }
    }
}

fn credential_failure(detail: impl Into<String>, resource: &str, code: Code) -> Failure {
    failure::envelope(
        POINT_CREDENTIAL_REDEEM,
        code,
        "one credential lookup",
        detail,
    )
    .with_context("resource", resource)
}

fn append_failure_cause(failure: Failure, cause: Option<Failure>) -> Failure {
    match cause {
        Some(cause) => failure.caused_by(cause),
        None => failure,
    }
}

async fn redeem_provider_resource(capability_id: &str, resource: &str) -> Result<Secret, Failure> {
    let id = capability_id.to_owned();
    let owned_resource = resource.to_owned();
    // Skarbiec uses blocking Unix I/O. Waiting for decryption must not occupy
    // Tokio's HTTP workers and starve /readyz during a credential sweep.
    tokio::task::spawn_blocking(move || {
        let binding = CapabilityRef::provider(&id, &owned_resource).map_err(|error| {
            credential_failure(
                format!("capability does not bind to `{owned_resource}`: {error}"),
                &owned_resource,
                Code::Config,
            )
        })?;
        let broker = client().ok_or_else(|| {
            credential_failure(
                "no capability broker client: SKARBIEC_CAP_SOCKET, SKARBIEC_WORKLOAD_ID, or the workload signing key is missing or unreadable",
                &owned_resource,
                Code::Config,
            )
        })?;
        broker.redeem(&binding).map_err(|error| {
            credential_failure(
                format!("authority refused capability redemption: {error}"),
                &owned_resource,
                failure::code_for("credential_unauthorized"),
            )
        })
    })
    .await
    .map_err(|error| credential_failure(format!("credential redemption worker failed: {error}"), resource, Code::Unknown))?
}

async fn redeem_subscription_credential(
    subscription_id: &str,
    provider: &str,
) -> Result<Secret, Failure> {
    let resource = subscription_resource(provider, subscription_id);
    match LOCAL_SUBSCRIPTION_CREDENTIALS.read() {
        Ok(credentials) => {
            if let Some(credential) = credentials.get(&resource) {
                return Ok(Secret::from_bytes(credential.as_slice().to_vec()));
            }
        }
        Err(_) => {
            return Err(credential_failure(
                "local subscription credential lock is poisoned",
                &resource,
                Code::Config,
            )
            .with_context("subscription", subscription_id)
            .with_context("provider", provider));
        }
    }

    let mut prior = None;
    if let Some(capability_id) = configured_capability(PROVIDER_CAPABILITIES_ENV, subscription_id) {
        match redeem_provider_resource(&capability_id, &resource).await {
            Ok(secret) => return Ok(secret),
            Err(refused) => prior = Some(refused),
        }
    }
    match issue_capability(PROVIDER_PURPOSE, &resource).await {
        Ok(fresh) => match redeem_provider_resource(&fresh, &resource).await {
            Ok(secret) => return Ok(secret),
            Err(refused) => prior = Some(append_failure_cause(refused, prior)),
        },
        Err(refused) => prior = Some(append_failure_cause(refused, prior)),
    }
    credential_by_grant(&resource).await.map_err(|refused| {
        append_failure_cause(refused, prior)
            .with_context("subscription", subscription_id)
            .with_context("provider", provider)
    })
}

/// Redeem one subscription credential at the final-use boundary. Expired
/// provider OAuth grants are refreshed only inside this scoped Brama runtime
/// and persisted before use. A rejected refresh or rejected vault write returns
/// its [`Failure`]; neither an expired grant nor an unpersisted rotation escapes.
pub async fn subscription_credential(
    subscription_id: &str,
    provider: &str,
) -> Result<Secret, Failure> {
    let credential = redeem_subscription_credential(subscription_id, provider).await?;
    if !super::oauth_refresh::needs_refresh(&credential, provider) {
        return Ok(credential);
    }
    drop(credential);
    refresh_subscription_credential_inner(subscription_id, provider, false).await
}
