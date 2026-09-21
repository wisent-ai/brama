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

use super::standalone::LOCAL_PROVIDER_CREDENTIALS;
use super::subscription::configured_subscription_ids;
use super::{capability_map, client, provider_resource};
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
            if subscription_ids.contains(&provider)
                || crate::subscription_dispatch::dispatch::is_subscription_provider(&provider)
            {
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

/// Configuration inspection must not issue credentials or block the HTTP worker.
/// Readiness and final-use redemption establish whether this declaration works.
pub fn provider_capability_configured(provider: &str) -> bool {
    !crate::providers::adapter::provider_requires_credential(provider)
        || configured_provider_capabilities().contains(provider)
        || provider_grant_routed(provider)
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
/// of the same authority, and caches the answer per provider: one child
/// process per provider per process, never one per request.
fn provider_grant_routed(provider: &str) -> bool {
    let resource = provider_resource(provider);
    if let Some(known) = GRANT_ROUTED
        .read()
        .ok()
        .and_then(|cache| cache.get(&resource).copied())
    {
        return known;
    }
    let routed = resolve_grant_route(&resource);
    if let Ok(mut cache) = GRANT_ROUTED.write() {
        cache.insert(resource, routed);
    }
    routed
}

static GRANT_ROUTED: LazyLock<RwLock<HashMap<String, bool>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

/// One `route resolve` for one resource, bounded, reading only whether the
/// vault answers a complete coordinate for it.
fn resolve_grant_route(resource: &str) -> bool {
    let Ok(mut child) = std::process::Command::new(super::vault::entitlements_router_bin())
        .arg("route")
        .arg("resolve")
        .arg(resource)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
    else {
        return false;
    };
    let deadline = std::time::Instant::now() + GRANT_PROBE_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return false;
            }
            Err(_) => return false,
        }
    }
    let Ok(output) = child.wait_with_output() else {
        return false;
    };
    if !output.status.success() {
        return false;
    }
    let Ok(document) = serde_json::from_slice::<serde_json::Value>(&output.stdout) else {
        return false;
    };
    document
        .get("routes")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|routes| {
            routes.iter().any(|route| {
                route.get("resource").and_then(serde_json::Value::as_str) == Some(resource)
                    && route.get("problem").is_none()
                    && route
                        .get("item")
                        .and_then(serde_json::Value::as_str)
                        .is_some_and(|item| !item.is_empty())
                    && route
                        .get("field")
                        .and_then(serde_json::Value::as_str)
                        .is_some_and(|field| !field.is_empty())
            })
        })
}

/// The probe is a local question to a local binary; a router that has not
/// answered by now is not going to change this answer.
const GRANT_PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
