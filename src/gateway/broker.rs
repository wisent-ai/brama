//! Credential seams for Brama.
//!
//! Capability redemption through the local Skarbiec broker is authoritative
//! for managed installations. A standalone desktop installation may instead
//! install an in-memory provider credential map before the server starts.
//!
//! This module owns credential coordinates, the order of final-use paths, and
//! the failure returned when neither produces a secret. `redemption` runs the
//! blocking authority exchange and classifies its errors; `vault` issues
//! capabilities and reads grants; `binding` reads deployment declarations;
//! `identity` resolves request-sign secrets; `standalone` owns the desktop
//! credential map; `subscription` manages paid accounts; and `rewards` is the
//! gateway's side of Wisent's contribution ledger.

mod binding;
mod configuration;
mod identity;
mod redemption;
pub mod rewards;
mod standalone;
mod subscription;
mod vault;

pub(crate) use vault::{login_seed_present, login_seed_states};
pub use vault::{decode_listing, VaultListItem};

use tracing::{info, warn};

use crate::capability::Secret;
use crate::core::failure::{self, POINT_CREDENTIAL_REDEEM};
use wisent_errors::{Code, Failure};

use binding::PROVIDER_CAPABILITIES_ENV;
use configuration::{capability_map, client, configured_capability};
use redemption::redeem_provider_resource;
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
    list_unroutable_accounts, owned_subscriptions, policy_unnamed_subscriptions,
    put_donated_credential, put_subscription_credential, put_subscription_credential_for_account,
    record_subscription_account, refresh_routing_subscriptions, refresh_subscription_credential,
    refresh_subscription_credential_ahead, remove_subscription_item,
    retry_disowned_subscription_credential, routing_subscriptions, subscription_account,
    subscription_tags_for_write, supports_oauth_refresh, DonationRefusal, RefreshAhead,
    SubscriptionEntry, UnroutableAccount,
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

/// Register account metadata only; an independent sign-in must supply its grant.
pub async fn register_discovered_account(
    provider: &str,
    id: &str,
    account: &str,
    metadata: &serde_json::Value,
) -> Result<(), String> {
    vault::register_discovered_account(provider, id, account, metadata).await
}

/// Restore the routing mark on a vault item that identifies an exact
/// account but lost it: the pool, the renewal sweep and Weles's login
/// resolution see the member again; no credential is written.
pub async fn restore_subscription_mark(
    provider: &str,
    id: &str,
    item: &str,
    account: &str,
) -> Result<(), String> {
    vault::restore_subscription_mark(provider, id, item, account).await
}

/// Redeem a direct provider API credential immediately before the HTTP call.
///
/// Managed launches issue at final use because a short-lived, single-use id
/// cannot be refreshed through a process environment. An optional seed is
/// accepted for standalone callers, but its refusal is steady state: obtain a
/// fresh capability and redeem that. Neither path holds plaintext beyond the
/// returned [`Secret`].
pub async fn provider_credential(provider: &str) -> Result<Secret, Failure> {
    if !crate::providers::adapter::provider_requires_credential(provider) {
        return Ok(Secret::from_bytes(Vec::new()));
    }
    let resource = provider_resource(provider);
    let result = if local_provider_credentials_enabled() {
        local_provider_credential(provider)
            .map_err(|detail| credential_failure(detail, &resource, Code::Config))
    } else {
        match configured_capability(PROVIDER_CAPABILITIES_ENV, provider) {
            Ok(seed) => obtain_credential(&resource, seed).await,
            Err(detail) => Err(credential_failure(detail, &resource, Code::Config)),
        }
    };
    result.map_err(|refused| {
        let refused = refused.with_context("provider", provider);
        warn!(
            event = "provider_credential_unavailable",
            provider,
            envelope = %refused.to_json(),
            "{}",
            refused.render()
        );
        refused
    })
}

/// The one cascade every managed credential comes through: a capability the
/// launcher configured, else a capability issued now, else the field-scoped
/// read grant -- each tried once, each refusal carried into the next.
///
/// Timed and logged here, once for both the direct and the subscription path.
/// The issue step is a child process to the entitlements router and the
/// redemption a broker round trip, and a request that took nine seconds used
/// to show only `routing_decision` and `routing_complete` nine seconds apart:
/// nothing in the log said whether the time went to this seam or to the
/// provider. `credential_obtained` says which of the three steps answered and
/// how long the whole cascade took, so a slow request can be attributed
/// without instrumenting a host.
async fn obtain_credential(resource: &str, configured: Option<String>) -> Result<Secret, Failure> {
    let started = std::time::Instant::now();
    let mut prior = None;
    if let Some(capability_id) = configured {
        match redeem_provider_resource(&capability_id, resource).await {
            Ok(secret) => {
                log_credential_obtained(resource, "configured", started);
                return Ok(secret);
            }
            Err(refused) => prior = Some(refused),
        }
    }
    match issue_capability(PROVIDER_PURPOSE, resource).await {
        Ok(fresh) => match redeem_provider_resource(&fresh, resource).await {
            Ok(secret) => {
                log_credential_obtained(resource, "issued", started);
                return Ok(secret);
            }
            Err(refused) => prior = Some(append_failure_cause(refused, prior)),
        },
        Err(refused) => prior = Some(append_failure_cause(refused, prior)),
    }
    let secret = credential_by_grant(resource)
        .await
        .map_err(|refused| append_failure_cause(refused, prior))?;
    log_credential_obtained(resource, "read-grant", started);
    Ok(secret)
}

fn log_credential_obtained(resource: &str, path: &str, started: std::time::Instant) {
    info!(
        event = "credential_obtained",
        resource,
        path,
        elapsed_ms = started.elapsed().as_millis() as u64,
        "credential obtained at final use"
    );
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

pub async fn redeem_subscription_credential(
    subscription_id: &str,
    provider: &str,
) -> Result<Secret, Failure> {
    let resource = subscription_resource(provider, subscription_id);
    if let Some(credential) = crate::core::sync::read_held(
        &LOCAL_SUBSCRIPTION_CREDENTIALS,
        "local subscription credentials",
    )
    .get(&resource)
    {
        return Ok(Secret::from_bytes(credential.as_slice().to_vec()));
    }

    let seed =
        configured_capability(PROVIDER_CAPABILITIES_ENV, subscription_id).map_err(|detail| {
            credential_failure(detail, &resource, Code::Config)
                .with_context("subscription", subscription_id)
                .with_context("provider", provider)
        })?;
    obtain_credential(&resource, seed).await.map_err(|refused| {
        refused
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
    if !subscription::is_borrowed(&credential)
        && !super::oauth_refresh::needs_refresh(&credential, provider).map_err(|refused| {
            refused
                .with_context("subscription", subscription_id)
                .with_context("provider", provider)
        })?
    {
        return Ok(credential);
    }
    drop(credential);
    refresh_subscription_credential_inner(subscription_id, provider, false).await
}
