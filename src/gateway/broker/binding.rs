//! Which providers this installation is configured to authenticate.
//!
//! Every answer here is about configuration rather than about a credential:
//! readiness, alias resolution and an authenticated model catalogue all need
//! to know what could be presented before anything is presented, and asking by
//! redeeming would cost a vault read per model in a catalogue with thousands
//! of entries. The two request paths -- a redeemed capability and a
//! field-scoped read grant -- must both be described, or this hides an alias
//! the request path can serve.

use std::collections::HashMap;
use std::sync::{LazyLock, RwLock};

use super::standalone::{local_provider_credentials_enabled, local_provider_names};
use super::subscription::configured_subscription_ids;
use super::{capability_map, provider_resource};
use crate::capability::CapabilityRef;

pub(super) const PROVIDER_CAPABILITIES_ENV: &str = "BRAMA_PROVIDER_CAPABILITY_IDS";

/// The providers this installation can authenticate directly, resolved once.
///
/// Capability redemption and a field-scoped read grant are the two request
/// paths. The catalogue must describe both; otherwise it hides an alias that
/// the request path can serve after a stale workload capability falls back to
/// the still-valid grant.
fn configured_provider_grants() -> Result<std::collections::HashSet<String>, String> {
    let Some(path) = std::env::var_os("SKARBIEC_CAPABILITY_ROUTES_FILE") else {
        return Ok(std::collections::HashSet::new());
    };
    let path = std::path::PathBuf::from(path);
    let raw = std::fs::read_to_string(&path)
        .map_err(|error| format!("read grant routes {}: {error}", path.display()))?;
    let table: serde_json::Value = serde_json::from_str(&raw)
        .map_err(|error| format!("decode grant routes {}: {error}", path.display()))?;
    // Skarbiec keeps the table as an object of resources and reads nothing
    // else; a `routes` key would be a resource of that name, not a wrapper.
    let routes = table.as_object().ok_or_else(|| {
        format!(
            "read grant routes {}: the table must be an object of resources, as Skarbiec writes it",
            path.display()
        )
    })?;
    Ok(routes
        .iter()
        .filter_map(|(resource, entry)| {
            let provider = resource.strip_prefix("provider:")?;
            // A subscription provider is served by the pool, one vault item
            // per account; a bare route for it names nothing the readiness
            // sweep could redeem, and asking anyway made Skarbiec refuse the
            // ambiguity on every start.
            if provider.is_empty()
                || provider.contains(':')
                || crate::subscription_dispatch::dispatch::is_subscription_provider(provider)
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
        .collect())
}

/// Resolve provider authentication once for an authenticated model catalogue.
///
/// Parsing the capability map and grant routes once avoids rebuilding the
/// workload client for every model in a catalogue with thousands of entries.
pub fn configured_provider_capabilities() -> Result<std::collections::HashSet<String>, String> {
    if local_provider_credentials_enabled() {
        return local_provider_names().map(|names| names.into_iter().collect());
    }
    let mut configured = configured_provider_grants()?;
    let subscription_ids = configured_subscription_ids()?;
    for (provider, capability_id) in capability_map(PROVIDER_CAPABILITIES_ENV)? {
        if subscription_ids.contains(&provider)
            || crate::subscription_dispatch::dispatch::is_subscription_provider(&provider)
        {
            continue;
        }
        let resource = provider_resource(&provider);
        CapabilityRef::provider(&capability_id, &resource).map_err(|error| {
            format!("validate {PROVIDER_CAPABILITIES_ENV} binding for `{resource}`: {error}")
        })?;
        configured.insert(provider);
    }
    Ok(configured)
}

/// Inspect configured credential routes without issuing a credential.
/// An unreadable authority is not evidence that a provider is absent.
pub fn provider_capability_configured(provider: &str) -> Result<bool, String> {
    if !crate::providers::adapter::provider_requires_credential(provider) {
        return Ok(true);
    }
    if local_provider_credentials_enabled() {
        return local_provider_names().map(|names| names.iter().any(|name| name == provider));
    }
    if configured_provider_capabilities()?.contains(provider) {
        return Ok(true);
    }
    provider_grant_routed(provider)
}

/// Whether the vault's own routing table answers this provider, asked of the
/// router that owns that table.
///
/// The environment variable above is the launcher's declaration, and a
/// managed gateway always has it. An operator shell never does — `brama
/// aliases` and `brama decide` are started by a person, not by the launcher —
/// and until this existed both answered `capability_absent` for a provider
/// whose credential the very next step would have read without trouble: the
/// presence check parsed a file named by a variable while the credential path
/// asked Skarbiec, so the two readers of one table disagreed exactly when no
/// launcher had spoken. This asks the same question the credential path asks,
/// of the same authority. Only an observed result is cached; a failed read
/// remains a refusal and a later request can observe recovery.
fn provider_grant_routed(provider: &str) -> Result<bool, String> {
    let resource = provider_resource(provider);
    if let Some(known) = crate::core::sync::read_held(&GRANT_ROUTED, "grant-route cache")
        .get(&resource)
        .copied()
    {
        return Ok(known);
    }
    let routed = resolve_grant_route(&resource)?;
    let mut observed = crate::core::sync::write_held(&GRANT_ROUTED, "grant-route cache");
    observed.insert(resource, routed);
    Ok(routed)
}

static GRANT_ROUTED: LazyLock<RwLock<HashMap<String, bool>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

/// One `route resolve` for one resource, run to its exit, reading only
/// whether the vault answers a complete coordinate for it. It runs as every
/// other vault child does: it stands in the readiness register while it
/// runs ([`super::vault::router::child_output_blocking`]), so a vault that
/// accepts it and never answers makes `/readyz` say so and ends this call
/// with the same sentence, instead of holding the request and the worker it
/// runs on.
fn resolve_grant_route(resource: &str) -> Result<bool, String> {
    let binary = super::vault::entitlements_router_bin()?;
    let operation = format!("`{binary} route resolve {resource}`");
    let output = super::vault::router::child_output_blocking(&binary, &operation, |command| {
        command.arg("route").arg("resolve").arg(resource);
    })?;
    if !output.status.success() {
        return Err(format!(
            "{operation} exited with {}: stdout: {}; stderr: {}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        ));
    }
    let document: serde_json::Value = serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("decode `{binary} route resolve {resource}` response: {error}"))?;
    let routes = document
        .get("routes")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| {
            format!("`{binary} route resolve {resource}` response has no routes array")
        })?;
    let mut configured = false;
    for route in routes {
        if route.get("resource").and_then(serde_json::Value::as_str) != Some(resource) {
            continue;
        }
        if let Some(problem) = route.get("problem") {
            return Err(format!(
                "`{binary} route resolve {resource}` refused: {problem}"
            ));
        }
        if !route
            .get("item")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|item| !item.is_empty())
            || !route
                .get("field")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|field| !field.is_empty())
        {
            return Err(format!(
                "`{binary} route resolve {resource}` returned an incomplete coordinate"
            ));
        }
        configured = true;
    }
    Ok(configured)
}
