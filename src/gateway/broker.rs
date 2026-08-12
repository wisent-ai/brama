//! Credential seams for Brama.
//!
//! Capability redemption through the local Skarbiec broker is authoritative
//! for managed installations. A standalone desktop installation may instead
//! install an in-memory provider credential map before the server starts.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{LazyLock, Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::{json, Value};
use tracing::warn;
use zeroize::{Zeroize, Zeroizing};

use crate::capability::{CapabilityClient, CapabilityRef, Secret};

const ENTITLEMENTS_ROUTER_BIN_ENV: &str = "ENTITLEMENTS_ROUTER_BIN";
const DEFAULT_ENTITLEMENTS_ROUTER_BIN: &str = "entitlements-router";
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
const PROVIDER_CAPABILITIES_ENV: &str = "BRAMA_PROVIDER_CAPABILITY_IDS";
const SUBSCRIPTION_CATALOG_ENV: &str = "BRAMA_SUBSCRIPTION_CATALOG";
const DONATED_SUBSCRIPTIONS_FILE_ENV: &str = "BRAMA_DONATED_SUBSCRIPTIONS_FILE";
const DEFAULT_DONATED_SUBSCRIPTIONS_FILE: &str = "/tmp/brama-skarbiec/donated-subscriptions.json";
const DONATION_RECIPIENT_ENV: &str = "SKARBIEC_DONATION_RECIPIENT";
const DEFAULT_DONATION_RECIPIENT: &str = "brama-service";

static OAUTH_REFRESH_LOCK: LazyLock<tokio::sync::Mutex<()>> =
    LazyLock::new(|| tokio::sync::Mutex::new(()));
static LOCAL_PROVIDER_CREDENTIALS: OnceLock<HashMap<String, Zeroizing<Vec<u8>>>> = OnceLock::new();

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

/// Install direct provider credentials supplied by the standalone desktop
/// launcher. The JSON input is consumed and zeroized; plaintext then remains
/// only in zeroizing process memory for this server lifetime.
pub fn install_local_provider_credentials(encoded: &mut String) -> Result<(), String> {
    let parsed: Result<HashMap<String, String>, _> = serde_json::from_str(encoded);
    encoded.zeroize();
    let mut parsed =
        parsed.map_err(|error| format!("local provider credentials are invalid: {error}"))?;
    if parsed
        .iter()
        .any(|(provider, value)| provider.trim().is_empty() || value.is_empty())
    {
        parsed.values_mut().for_each(Zeroize::zeroize);
        return Err("local provider names and credentials must be non-empty".to_owned());
    }
    let mut credentials = HashMap::with_capacity(parsed.len());
    for (provider, mut value) in parsed {
        credentials.insert(
            provider.trim().to_owned(),
            Zeroizing::new(value.as_bytes().to_vec()),
        );
        value.zeroize();
    }
    LOCAL_PROVIDER_CREDENTIALS
        .set(credentials)
        .map_err(|_| "local provider credentials were already installed".to_owned())
}

fn local_provider_credential(provider: &str) -> Option<Secret> {
    LOCAL_PROVIDER_CREDENTIALS
        .get()?
        .get(provider)
        .map(|value| Secret::from_bytes(value.as_slice().to_vec()))
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

/// Resolve an agent-specific request-signing secret immediately before HMAC
/// verification. Echo, legacy Content Platform, Oko, and Weles are strict
/// central-item projections; they never fall back to generated agent resources
/// or another product.
pub async fn get_agent_auth_secret(agent_id: &str) -> Option<Secret> {
    if CENTRAL_REQUEST_SIGN_AGENTS.contains(&agent_id) {
        let secret = capability_map(REQUEST_SIGN_IDENTITIES_ENV)?.remove(agent_id)?;
        return Some(Secret::from_bytes(secret.into_bytes()));
    }

    let resource = format!("agent:{}", slug(agent_id));
    if let Some(capability_id) = configured_capability(REQUEST_SIGN_CAPABILITIES_ENV, agent_id) {
        if let Ok(binding) = CapabilityRef::request_sign(&capability_id, &resource) {
            if let Some(secret) = client()?.redeem(&binding).ok() {
                return Some(secret);
            }
        }
    }
    // Same reasoning as a provider credential: the id the launcher seeded is
    // short-lived by contract, so its refusal is the steady state and a fresh
    // capability is the answer, not an error.
    // The provider path already says when the authority refuses; this one
    // returned None in silence, and the caller sees only "no auth secret for
    // agent" -- a sentence that fits a missing item, a refused issue and a
    // denied redemption equally well.
    let Some(fresh) = issue_capability(REQUEST_SIGN_PURPOSE, &resource).await else {
        warn!(
            event = "request_sign_issue_failed",
            agent_id, %resource,
            "the authority would not issue a request-sign capability; trying the read grant"
        );
        return credential_by_grant(&resource).await;
    };
    let Ok(binding) = CapabilityRef::request_sign(&fresh, &resource) else {
        warn!(
            event = "request_sign_binding_invalid",
            agent_id, %resource, "the issued capability does not bind to this resource"
        );
        return None;
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
            // The provider path already falls back this way when a capability
            // cannot be redeemed -- a workload proof that no longer matches the
            // key the vault registered takes down every redemption, and there
            // is no reason the signing secret should be the one credential with
            // no way through. The grant is the authority's own, narrower than a
            // capability, and the route is the same coordinate either way.
            credential_by_grant(&resource).await
        }
    }
}
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
    let mut configured: std::collections::HashSet<String> = LOCAL_PROVIDER_CREDENTIALS
        .get()
        .map(|credentials| credentials.keys().cloned().collect())
        .unwrap_or_default();
    configured.extend(configured_provider_grants());
    if client().is_none() {
        return configured;
    }
    if let Some(map) = capability_map(PROVIDER_CAPABILITIES_ENV) {
        for (provider, capability_id) in map {
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
/// [`provider_credential`], which falls back to the exact field-scoped route
/// when capability issuance or redemption is unavailable.
pub fn provider_capability_configured(provider: &str) -> bool {
    if LOCAL_PROVIDER_CREDENTIALS
        .get()
        .is_some_and(|credentials| credentials.contains_key(provider))
    {
        return true;
    }
    let resource = provider_resource(provider);
    if capability_route(&resource).is_some() {
        return true;
    }
    if client().is_none() {
        return false;
    }
    if let Some(capability_id) = configured_capability(PROVIDER_CAPABILITIES_ENV, provider) {
        if CapabilityRef::provider(&capability_id, &resource).is_ok() {
            return true;
        }
    }
    issue_capability_blocking(PROVIDER_PURPOSE, &resource)
        .is_some_and(|capability_id| CapabilityRef::provider(&capability_id, &resource).is_ok())
}

/// Redeem a direct provider API credential immediately before the HTTP call.
///
/// The launcher seeds one id per provider at boot and those expire within the
/// hour, so a refusal is the expected steady state rather than a fault: obtain
/// a fresh capability and redeem that. Both attempts go through the same
/// broker, and neither ever holds plaintext beyond the returned [`Secret`].
pub async fn provider_credential(provider: &str) -> Option<Secret> {
    if let Some(secret) = local_provider_credential(provider) {
        return Some(secret);
    }
    let resource = provider_resource(provider);
    // Every step below can fail, and this used to return None for all of them,
    // so a caller saw one sentence -- "credential is unavailable" -- covering a
    // broker that was never configured, a capability that had expired, a route
    // the authority does not have, and a workload the broker will not accept.
    // Telling them apart from the outside was not possible, and guessing wrong
    // cost a day.
    let Some(broker) = client() else {
        warn!(
            event = "provider_credential_no_broker",
            provider,
            "no capability broker client: SKARBIEC_CAP_SOCKET, SKARBIEC_WORKLOAD_ID or the \
             workload signing key is missing or unreadable; trying the read grant"
        );
        return credential_by_grant(&resource).await;
    };
    if let Some(capability_id) = configured_capability(PROVIDER_CAPABILITIES_ENV, provider) {
        match CapabilityRef::provider(&capability_id, &resource) {
            Ok(binding) => match broker.redeem(&binding) {
                Ok(secret) => return Some(secret),
                Err(error) => warn!(
                    event = "provider_credential_redeem_failed",
                    provider,
                    %resource,
                    %error,
                    "the capability issued at boot did not redeem; asking for a fresh one"
                ),
            },
            Err(error) => warn!(
                event = "provider_credential_binding_invalid",
                provider, %resource, %error, "the configured capability id is not usable"
            ),
        }
    } else {
        warn!(
            event = "provider_credential_not_configured",
            provider, "no capability was issued for this provider at boot; asking for one now"
        );
    }
    let Some(fresh) = issue_capability(PROVIDER_PURPOSE, &resource).await else {
        warn!(
            event = "provider_credential_issue_failed",
            provider, %resource,
            "the authority would not issue a capability for this resource; trying the read grant"
        );
        return credential_by_grant(&resource).await;
    };
    let binding = CapabilityRef::provider(&fresh, &resource).ok()?;
    match broker.redeem(&binding) {
        Ok(secret) => Some(secret),
        Err(error) => {
            warn!(
                event = "provider_credential_fresh_redeem_failed",
                provider, %resource, %error,
                "a freshly issued capability did not redeem either; trying the read grant"
            );
            credential_by_grant(&resource).await
        }
    }
}

/// Redeem one provider-purpose capability for a resource.
///
/// Kept as a function rather than a chain so the capability id outlives the
/// binding that borrows it; inlining it is what the borrow checker refuses.
///
/// The refusal is logged rather than swallowed. The direct-provider path above
/// already says why a redemption failed, and the subscription path returning a
/// bare `None` is what made "credential unavailable" cover a missing route, an
/// expired capability and a workload the vault will not accept -- three repairs
/// behind one sentence.
fn redeem_provider_resource(capability_id: &str, resource: &str) -> Option<Secret> {
    let binding = match CapabilityRef::provider(capability_id, resource) {
        Ok(binding) => binding,
        Err(error) => {
            warn!(
                event = "subscription_capability_binding_invalid",
                %resource, %error, "the capability id does not bind to this resource"
            );
            return None;
        }
    };
    let broker = match client() {
        Some(broker) => broker,
        None => {
            warn!(
                event = "subscription_capability_no_broker",
                %resource,
                "no capability broker client: SKARBIEC_CAP_SOCKET, SKARBIEC_WORKLOAD_ID or the \
                 workload signing key is missing or unreadable"
            );
            return None;
        }
    };
    match broker.redeem(&binding) {
        Ok(secret) => Some(secret),
        Err(error) => {
            warn!(
                event = "subscription_capability_redeem_refused",
                %resource, %error, "the authority refused to redeem this capability"
            );
            None
        }
    }
}

async fn redeem_subscription_credential(subscription_id: &str, provider: &str) -> Option<Secret> {
    let resource = subscription_resource(provider, subscription_id);
    // A capability is single-use and short-lived by contract, and model
    // discovery redeems one at boot, so the id the launcher seeded is spent
    // before the first request arrives. Ask for a fresh capability and fall
    // back to the authority's own grant when the seeded one is gone.
    let seeded = configured_capability(PROVIDER_CAPABILITIES_ENV, subscription_id);
    match seeded
        .as_deref()
        .and_then(|capability_id| redeem_provider_resource(capability_id, &resource))
    {
        Some(credential) => Some(credential),
        None => match issue_capability(PROVIDER_PURPOSE, &resource).await {
            Some(fresh) => match redeem_provider_resource(&fresh, &resource) {
                Some(credential) => Some(credential),
                None => {
                    warn!(
                        event = "subscription_credential_redeem_failed",
                        provider, %resource,
                        "a freshly issued capability did not redeem; trying the read grant"
                    );
                    credential_by_grant(&resource).await
                }
            },
            None => {
                warn!(
                    event = "subscription_credential_issue_failed",
                    provider, %resource,
                    "the authority would not issue a capability; trying the read grant"
                );
                credential_by_grant(&resource).await
            }
        },
    }
}

async fn refresh_subscription_credential_inner(
    subscription_id: &str,
    provider: &str,
    force: bool,
    preserve_on_failure: bool,
) -> Option<Secret> {
    // Refresh-token rotation is single-flight. Re-reading after the lock lets a
    // concurrent caller observe the value already written to the vault.
    let _guard = OAUTH_REFRESH_LOCK.lock().await;
    let credential = redeem_subscription_credential(subscription_id, provider).await?;
    if !force && !super::oauth_refresh::needs_refresh(&credential, provider) {
        return Some(credential);
    }
    let mut fresh = match super::oauth_refresh::refresh(&credential, provider).await {
        Ok(fresh) => fresh,
        Err(error) => {
            warn!(
                event = "oauth_refresh_failed",
                provider, %error, "OAuth refresh failed"
            );
            return preserve_on_failure.then_some(credential);
        }
    };
    if put_subscription_credential(subscription_id, provider, &fresh)
        .await
        .is_err()
    {
        warn!(
            event = "oauth_refresh_persist_failed",
            provider, "refreshed OAuth credential could not be persisted; using it in memory"
        );
    }
    Some(Secret::from_bytes(std::mem::take(&mut *fresh)))
}

/// Redeem one subscription credential at the final-use boundary. Expired
/// provider OAuth grants are refreshed only inside this scoped Brama runtime,
/// used immediately, and persisted through the local entitlements router when
/// possible.
pub async fn subscription_credential(subscription_id: &str, provider: &str) -> Option<Secret> {
    let credential = redeem_subscription_credential(subscription_id, provider).await?;
    if !super::oauth_refresh::needs_refresh(&credential, provider) {
        return Some(credential);
    }
    drop(credential);
    refresh_subscription_credential_inner(subscription_id, provider, false, true).await
}

/// Force one OAuth refresh after the provider rejects a grant whose local
/// expiry still claims it is valid. The rejected grant is not returned when
/// refresh fails because retrying it would only repeat the provider error.
pub async fn refresh_subscription_credential(
    subscription_id: &str,
    provider: &str,
) -> Option<Secret> {
    refresh_subscription_credential_inner(subscription_id, provider, true, false).await
}

/// Enumerate one agent's subscription metadata through the local entitlements
/// broker or its trusted deployment-time catalog. The donated-subscriptions
/// overlay is metadata only; every credential use still requires a capability.
pub async fn list_subscriptions(agent_id: &str) -> Vec<SubscriptionEntry> {
    let mut entries = list_subscriptions_result(agent_id)
        .await
        .unwrap_or_default();
    for donated in donated_subscriptions(agent_id) {
        match entries.iter_mut().find(|entry| entry.id == donated.id) {
            Some(existing) => *existing = donated,
            None => entries.push(donated),
        }
    }
    entries
}

/// Path of the donated-subscriptions overlay file.
pub fn donated_subscriptions_path() -> PathBuf {
    std::env::var(DONATED_SUBSCRIPTIONS_FILE_ENV)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_DONATED_SUBSCRIPTIONS_FILE))
}

/// Overlay entries for one agent. A missing or corrupt file yields nothing.
fn donated_subscriptions(agent_id: &str) -> Vec<SubscriptionEntry> {
    let Ok(text) = std::fs::read_to_string(donated_subscriptions_path()) else {
        return Vec::new();
    };
    parse_subscriptions(text.as_bytes(), agent_id).unwrap_or_default()
}

/// Read-modify-write the overlay file atomically (temp + rename, mode 0600).
fn update_donated_items(update: impl FnOnce(&mut Vec<Value>)) -> Result<(), String> {
    let path = donated_subscriptions_path();
    let mut items = match std::fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str::<Value>(&text)
            .ok()
            .and_then(|value| value.get("items").and_then(Value::as_array).cloned())
            .ok_or_else(|| "donated subscriptions file is corrupt".to_string())?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(error) => return Err(format!("read donated subscriptions file: {error}")),
    };
    update(&mut items);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("create donated subscriptions dir: {error}"))?;
    }
    let payload = serde_json::to_string_pretty(&json!({"items": items}))
        .map_err(|error| format!("encode donated subscriptions: {error}"))?;
    let tmp = path.with_extension("tmp");
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp)
            .map_err(|error| format!("write donated subscriptions file: {error}"))?;
        file.write_all(payload.as_bytes())
            .and_then(|()| file.sync_all())
            .map_err(|error| format!("write donated subscriptions file: {error}"))?;
    }
    std::fs::rename(&tmp, &path)
        .map_err(|error| format!("replace donated subscriptions file: {error}"))?;
    Ok(())
}

/// Record one donated subscription in the overlay file.
pub fn donated_add(agent_id: &str, id: &str, provider: &str) -> Result<(), String> {
    update_donated_items(|items| {
        items.retain(|item| item.get("id").and_then(Value::as_str) != Some(id));
        items.push(json!({
            "id": id,
            "provider": provider,
            "agent_id": agent_id,
            "status": "active",
        }));
    })
}

/// Drop one subscription id from the overlay file (no-op when absent).
pub fn donated_remove(id: &str) -> Result<(), String> {
    update_donated_items(|items| {
        items.retain(|item| item.get("id").and_then(Value::as_str) != Some(id));
    })
}

fn entitlements_router_bin() -> String {
    std::env::var(ENTITLEMENTS_ROUTER_BIN_ENV)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_ENTITLEMENTS_ROUTER_BIN.to_owned())
}

const PROVIDER_PURPOSE: &str = "brama.provider.authenticate";
const REQUEST_SIGN_PURPOSE: &str = "brama.request.sign";
/// The agent a capability is issued to, and the identity whose registered key
/// the authority verifies a redemption against.
///
/// It is deliberately not `brama-service`. That consumer exists, but it holds
/// one `read` capability for this gateway's GPG key, and the authority allows
/// a workload key only on a consumer that carries `acquire` -- so naming it
/// here can never redeem, whatever else is fixed. The runtime agent needs its
/// own acquisition consumer in the vault, bound to the proof key this
/// installation provisions.
const RUNTIME_AGENT: &str = "brama-runtime";
const CAPABILITY_TARGET: &str = "brama";

/// The `capability-issue` request, taking the broker's own limits.
///
/// No lifetime or use count is passed, and that is the point. Skarbiec refuses
/// a ttl over an hour and a use count over sixteen, while the launcher asked
/// for thirty days and a million uses: every request was refused and the
/// gateway came up answering `/health` and serving nothing. The broker's
/// defaults are a short life and a single use, which is the right shape once
/// the capability is obtained where it is spent. Nothing is cached: a
/// single-use capability has nothing worth keeping, and an id the launcher put
/// in the environment of a running process cannot be refreshed at all.
fn issue_arguments(purpose: &str, resource: &str) -> Vec<String> {
    vec![
        "capability-issue".to_owned(),
        "--agent".to_owned(),
        RUNTIME_AGENT.to_owned(),
        "--purpose".to_owned(),
        purpose.to_owned(),
        "--resource".to_owned(),
        resource.to_owned(),
        "--target".to_owned(),
        CAPABILITY_TARGET.to_owned(),
    ]
}

fn issued_capability_id(
    stdout: &[u8],
    stderr: &[u8],
    succeeded: bool,
    resource: &str,
) -> Option<String> {
    if !succeeded {
        warn!(
            event = "capability_issue_refused",
            resource = resource,
            detail = String::from_utf8_lossy(stderr).trim()
        );
        return None;
    }
    serde_json::from_slice::<Value>(stdout)
        .ok()?
        .get("capability_id")
        .and_then(Value::as_str)
        .map(str::to_owned)
}

/// Obtain one capability on the request path.
async fn issue_capability(purpose: &str, resource: &str) -> Option<String> {
    let output = tokio::process::Command::new(entitlements_router_bin())
        .args(issue_arguments(purpose, resource))
        .output()
        .await
        .ok()?;
    issued_capability_id(
        &output.stdout,
        &output.stderr,
        output.status.success(),
        resource,
    )
}

/// The vault coordinate a resource stands for, as the operator wrote it.
///
/// The same table the authority consults, read here so nothing in this process
/// ever decides for itself which credential a purpose means.
fn capability_route(resource: &str) -> Option<(String, String)> {
    let path = std::env::var_os("SKARBIEC_CAPABILITY_ROUTES_FILE")?;
    let raw = std::fs::read_to_string(path).ok()?;
    let document: serde_json::Value = serde_json::from_str(&raw).ok()?;
    let table = document.get("routes").unwrap_or(&document);
    let entry = table.get(resource)?;
    let item = entry.get("item")?.as_str()?.to_owned();
    let field = entry.get("field")?.as_str()?.to_owned();
    Some((item, field))
}

/// Read one provider credential through the grant the vault already carries.
///
/// Redeeming a capability is the stronger path and stays first. It is not the
/// only one the fleet provisions: some providers are granted as a plain
/// per-field read to a named consumer -- `read:provider:local-openai#token` is
/// exactly that -- and where the grant that exists is a read, refusing to use
/// it means refusing to serve a provider the operator deliberately allowed.
///
/// Nothing is widened here. The router presents this host's consumer identity
/// and the authority still decides: without the grant the read is refused, and
/// the coordinate comes from the operator's routes table rather than a guess.
async fn credential_by_grant(resource: &str) -> Option<Secret> {
    let Some((item, field)) = capability_route(resource) else {
        warn!(
            event = "credential_read_unrouted",
            %resource,
            "no capability route maps this resource to a vault item and field"
        );
        return None;
    };
    let output = match tokio::process::Command::new(entitlements_router_bin())
        .arg("get")
        .arg(&item)
        .output()
        .await
    {
        Ok(output) => output,
        Err(error) => {
            warn!(
                event = "credential_read_unspawnable",
                %resource, %item, %error,
                "the entitlements router could not be started"
            );
            return None;
        }
    };
    if !output.status.success() {
        // The authority's own sentence, not a summary of it. "credential
        // unavailable" fits a missing item, a key this host lacks and an item
        // still in the legacy envelope equally well, and only the last of those
        // is fixed by a migration the message would otherwise never name.
        let detail = String::from_utf8_lossy(&output.stderr);
        warn!(
            event = "credential_read_refused",
            %resource,
            %item,
            detail = %detail.trim(),
            "the authority refused a direct read of this credential"
        );
        return None;
    }
    let payload: serde_json::Value = match serde_json::from_slice(&output.stdout) {
        Ok(payload) => payload,
        Err(error) => {
            warn!(
                event = "credential_read_unparseable",
                %resource, %item, %error,
                "the authority returned a credential document that is not JSON"
            );
            return None;
        }
    };
    let Some(value) = payload.get("fields").and_then(|fields| fields.get(&field)) else {
        warn!(
            event = "credential_read_field_absent",
            %resource, %item, %field,
            "the credential document carries no such field"
        );
        return None;
    };
    let bytes = match value {
        serde_json::Value::String(value) => value.as_bytes().to_vec(),
        serde_json::Value::Null => {
            warn!(
                event = "credential_read_field_absent",
                %resource, %item, %field,
                "the credential document carries a null field"
            );
            return None;
        }
        value => match serde_json::to_vec(value) {
            Ok(bytes) => bytes,
            Err(error) => {
                warn!(
                    event = "credential_read_field_unserializable",
                    %resource, %item, %field, %error,
                    "the credential field could not be preserved as JSON"
                );
                return None;
            }
        },
    };
    Some(Secret::from_bytes(bytes))
}

/// The same request during startup validation, where there is no runtime to
/// await on and blocking for one child process costs nothing.
fn issue_capability_blocking(purpose: &str, resource: &str) -> Option<String> {
    let output = std::process::Command::new(entitlements_router_bin())
        .args(issue_arguments(purpose, resource))
        .output()
        .ok()?;
    issued_capability_id(
        &output.stdout,
        &output.stderr,
        output.status.success(),
        resource,
    )
}

fn donation_recipient() -> String {
    std::env::var(DONATION_RECIPIENT_ENV)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_DONATION_RECIPIENT.to_owned())
}

async fn put_credential(item_id: &str, secret: &[u8]) -> Result<(), String> {
    use std::process::Stdio;
    use tokio::io::AsyncWriteExt;

    let mut child = tokio::process::Command::new(entitlements_router_bin())
        .arg("credential-put")
        .arg(item_id)
        .arg("--recipient")
        .arg(donation_recipient())
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("spawn entitlements router: {error}"))?;
    let write_result = match child.stdin.take() {
        Some(mut stdin) => stdin.write_all(secret).await,
        None => return Err("entitlements router stdin is unavailable".to_owned()),
    };
    if write_result.is_err() {
        let _ = child.kill().await;
        return Err("write entitlements router credential failed".to_owned());
    }
    let output = child
        .wait_with_output()
        .await
        .map_err(|error| format!("wait entitlements router: {error}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let detail_limit: usize = "200".parse().expect("valid credential error detail limit");
        let detail: String = stderr.trim().chars().take(detail_limit).collect();
        return Err(format!("credential-put failed: {detail}"));
    }
    Ok(())
}

/// Store one donated OAuth credential blob through the local entitlements
/// router; plaintext crosses only the child process stdin pipe.
pub async fn put_donated_credential(subscription_id: &str, api_key: &str) -> Result<(), String> {
    let item_id = format!("provider:claude-code:{subscription_id}");
    put_credential(&item_id, api_key.as_bytes()).await
}

async fn put_subscription_credential(
    subscription_id: &str,
    provider: &str,
    credential: &[u8],
) -> Result<(), String> {
    let item_id = format!("provider:{}:{}", slug(provider), slug(subscription_id));
    put_credential(&item_id, credential).await
}

fn complete_field(value: Option<String>) -> Option<String> {
    value.filter(|field| !field.is_empty() && field.trim() == field)
}

fn parse_subscriptions(output: &[u8], agent_id: &str) -> Result<Vec<SubscriptionEntry>, ()> {
    let response: BrokerItems = serde_json::from_slice(output).map_err(|_| ())?;

    Ok(response
        .items
        .into_iter()
        .filter_map(|entry| {
            let id = complete_field(entry.id)?;
            let provider = complete_field(entry.provider)?;
            let entry_agent_id = complete_field(entry.agent_id)?;
            let status = complete_field(entry.status)?;
            if entry_agent_id != agent_id {
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

fn configured_subscriptions(agent_id: &str) -> Option<Result<Vec<SubscriptionEntry>, ()>> {
    let encoded = std::env::var(SUBSCRIPTION_CATALOG_ENV).ok()?;
    Some(parse_subscriptions(encoded.as_bytes(), agent_id))
}

/// One vault item row from the entitlements router's bare `list` command.
#[derive(Debug, Deserialize)]
struct VaultListItem {
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    deleted: bool,
}

fn subscription_tag_value<'a>(tags: &'a [String], prefix: &str) -> Option<&'a str> {
    tags.iter().find_map(|tag| {
        tag.strip_prefix(prefix)
            .and_then(|value| (!value.is_empty()).then_some(value))
    })
}

/// Map the router's full vault listing to one agent's subscriptions. An item
/// is a subscription when it carries the `brama:subscription` tag plus
/// `brama:agent:<agent>`; `brama:provider:` and `brama:id:` tags carry the
/// provider and subscription id, so item ids stay opaque and renames are safe.
/// Non-deleted resources become active entries.
fn parse_live_subscriptions(output: &[u8], agent_id: &str) -> Result<Vec<SubscriptionEntry>, ()> {
    let agent_tag = format!("brama:agent:{agent_id}");
    let items: Vec<VaultListItem> = serde_json::from_slice(output).map_err(|_| ())?;
    Ok(items
        .into_iter()
        .filter(|item| !item.deleted)
        .filter(|item| {
            item.tags.iter().any(|tag| tag == "brama:subscription")
                && item.tags.iter().any(|tag| tag == &agent_tag)
        })
        .filter_map(|item| {
            Some(SubscriptionEntry {
                id: subscription_tag_value(&item.tags, "brama:id:")?.to_owned(),
                provider: subscription_tag_value(&item.tags, "brama:provider:")?
                    .trim()
                    .to_lowercase()
                    .replace('_', "-"),
                status: "active".to_owned(),
            })
        })
        .collect())
}

/// Live discovery results per agent. Entries are stored only after a
/// successful listing so a failed shell never poisons the cache.
fn live_subscriptions_cache() -> &'static Mutex<HashMap<String, (Instant, Vec<SubscriptionEntry>)>>
{
    static CACHE: OnceLock<Mutex<HashMap<String, (Instant, Vec<SubscriptionEntry>)>>> =
        OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

const LIVE_SUBSCRIPTIONS_CACHE_TTL: Duration = Duration::from_secs(60);

/// Resolve one agent's subscriptions from the vault, serving a fresh cached
/// listing unless `bypass_cache` is set (used when a lookup failed and the
/// caller wants to re-check the vault instead of trusting a stale entry).
async fn live_subscriptions(
    broker: &str,
    agent_id: &str,
    bypass_cache: bool,
) -> Result<Vec<SubscriptionEntry>, ()> {
    if !bypass_cache {
        if let Ok(cache) = live_subscriptions_cache().lock() {
            if let Some((fetched_at, entries)) = cache.get(agent_id) {
                if fetched_at.elapsed() < LIVE_SUBSCRIPTIONS_CACHE_TTL {
                    return Ok(entries.clone());
                }
            }
        }
    }
    let entries = list_subscriptions_live(broker, agent_id).await?;
    if let Ok(mut cache) = live_subscriptions_cache().lock() {
        cache.insert(agent_id.to_owned(), (Instant::now(), entries.clone()));
    }
    Ok(entries)
}

/// Shell the entitlements router's bare `list`, which returns a JSON array of
/// every vault item (`{"id","type","tags","updated_at","deleted","versions"}`).
async fn list_subscriptions_live(
    broker: &str,
    agent_id: &str,
) -> Result<Vec<SubscriptionEntry>, ()> {
    let output = tokio::process::Command::new(broker)
        .arg("list")
        .output()
        .await
        .map_err(|_| ())?;
    if !output.status.success() {
        return Err(());
    }
    parse_live_subscriptions(&output.stdout, agent_id)
}

async fn list_subscriptions_result(agent_id: &str) -> Result<Vec<SubscriptionEntry>, ()> {
    let broker = entitlements_router_bin();
    // Live vault discovery supplies conventional per-agent entries. The
    // deployment catalog additionally carries explicit sharing decisions for
    // items whose owner-prefixed id belongs to a different agent. It was built
    // from the same live listing at startup, so merging it cannot invent a
    // credential; final use still requires capability redemption.
    if let Ok(mut live) = live_subscriptions(&broker, agent_id, false).await {
        if let Some(Ok(configured)) = configured_subscriptions(agent_id) {
            for entry in configured {
                if !live.iter().any(|existing| existing.id == entry.id) {
                    live.push(entry);
                }
            }
        }
        return Ok(live);
    }
    configured_subscriptions(agent_id).unwrap_or(Err(()))
}
