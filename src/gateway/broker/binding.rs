//! Which providers this installation is configured to authenticate.
//!
//! Every answer here is about configuration rather than about a credential:
//! readiness, alias resolution and an authenticated model catalogue all need
//! to know what could be presented before anything is presented, and asking by
//! redeeming would cost a vault read per model in a catalogue with thousands
//! of entries. The two request paths -- a redeemed capability and a
//! field-scoped read grant -- must both be described, or this hides an alias
//! the request path can serve.

use tracing::warn;

use super::standalone::LOCAL_PROVIDER_CREDENTIALS;
use super::subscription::configured_subscription_ids;
use super::vault::{capability_route, issue_capability_blocking, PROVIDER_PURPOSE};
use super::{capability_map, client, configured_capability, provider_resource};
use crate::capability::CapabilityRef;

pub(super) const PROVIDER_CAPABILITIES_ENV: &str = "BRAMA_PROVIDER_CAPABILITY_IDS";

/// The providers this installation can authenticate directly, resolved once.
///
/// Capability redemption and a field-scoped read grant are the two request
/// paths. The catalogue must describe both; otherwise it hides an alias that
/// the request path can serve after a stale workload capability falls back to
/// the still-valid grant.
fn configured_provider_grants() -> std::collections::HashSet<String> {
    let Some(path) = std::env::var_os("SKARBIEC_CAPABILITY_ROUTES_FILE") else {
        return std::collections::HashSet::new();
    };
    let Ok(raw) = std::fs::read_to_string(path) else {
        return std::collections::HashSet::new();
    };
    let Ok(table) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return std::collections::HashSet::new();
    };
    let routes = table.get("routes").unwrap_or(&table);
    let Some(routes) = routes.as_object() else {
        return std::collections::HashSet::new();
    };
    routes
        .iter()
        .filter_map(|(resource, entry)| {
            let provider = resource.strip_prefix("provider:")?;
            if provider.is_empty()
                || provider.contains(':')
                || entry
                    .get("item")
                    .and_then(serde_json::Value::as_str)
                    .is_none()
                || entry
                    .get("field")
                    .and_then(serde_json::Value::as_str)
                    .is_none()
            {
                return None;
            }
            Some(provider.to_owned())
        })
        .collect()
}

/// Resolve provider authentication once for an authenticated model catalogue.
///
/// Parsing the capability map and grant routes once avoids rebuilding the
/// workload client for every model in a catalogue with thousands of entries.
pub fn configured_provider_capabilities() -> std::collections::HashSet<String> {
    if let Some(configured) = LOCAL_PROVIDER_CREDENTIALS
        .read()
        .ok()
        .and_then(|credentials| {
            credentials
                .as_ref()
                .map(|values| values.keys().cloned().collect())
        })
    {
        return configured;
    }
    let mut configured = configured_provider_grants();
    if client().is_none() {
        return configured;
    }
    let subscription_ids = configured_subscription_ids();
    if let Some(map) = capability_map(PROVIDER_CAPABILITIES_ENV) {
        for (provider, capability_id) in map {
            if subscription_ids.contains(&provider) {
                continue;
            }
            let resource = provider_resource(&provider);
            if CapabilityRef::provider(&capability_id, &resource).is_ok() {
                configured.insert(provider);
            }
        }
    }
    configured
}

/// Return whether this installation has a direct capability or read grant.
///
/// Startup and alias resolution must ask the same question as
/// [`super::provider_credential`], which falls back to the exact field-scoped
/// route when capability issuance or redemption is unavailable.
pub fn provider_capability_configured(provider: &str) -> bool {
    if !crate::providers::adapter::provider_requires_credential(provider) {
        return true;
    }
    if let Ok(credentials) = LOCAL_PROVIDER_CREDENTIALS.read() {
        if let Some(credentials) = credentials.as_ref() {
            return credentials.contains_key(provider);
        }
    }
    let resource = provider_resource(provider);
    if capability_route(&resource).is_ok() {
        return true;
    }
    if let Some(capability_id) = configured_capability(PROVIDER_CAPABILITIES_ENV, provider) {
        if client().is_some() && CapabilityRef::provider(&capability_id, &resource).is_ok() {
            return true;
        }
    }
    if client().is_none() {
        return false;
    }
    match issue_capability_blocking(PROVIDER_PURPOSE, &resource) {
        Ok(capability_id) => CapabilityRef::provider(&capability_id, &resource).is_ok(),
        Err(refused) => {
            warn!(
                event = "provider_capability_check_failed",
                provider,
                envelope = %refused.to_json(),
                "{}",
                refused.render()
            );
            false
        }
    }
}
