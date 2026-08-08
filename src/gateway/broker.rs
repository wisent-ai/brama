//! Credential seams for Brama.
//!
//! Capability redemption through the local Skarbiec broker is authoritative.
//! Missing, malformed, or unavailable capability configuration fails closed;
//! Brama never falls back to an ambient secret or remote credential store.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{LazyLock, Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::{json, Value};
use tracing::warn;

use crate::capability::{CapabilityClient, CapabilityRef, Secret};

const ENTITLEMENTS_ROUTER_BIN_ENV: &str = "ENTITLEMENTS_ROUTER_BIN";
const DEFAULT_ENTITLEMENTS_ROUTER_BIN: &str = "entitlements-router";
const REQUEST_SIGN_CAPABILITIES_ENV: &str = "BRAMA_REQUEST_SIGN_CAPABILITY_IDS";
const REQUEST_SIGN_IDENTITIES_ENV: &str = "BRAMA_REQUEST_SIGN_IDENTITIES";
const CENTRAL_REQUEST_SIGN_AGENTS: &[&str] = &["echo", "content-platform", "oko", "weles"];
const PROVIDER_CAPABILITIES_ENV: &str = "BRAMA_PROVIDER_CAPABILITY_IDS";
const SUBSCRIPTION_CATALOG_ENV: &str = "BRAMA_SUBSCRIPTION_CATALOG";
const DONATED_SUBSCRIPTIONS_FILE_ENV: &str = "BRAMA_DONATED_SUBSCRIPTIONS_FILE";
const DEFAULT_DONATED_SUBSCRIPTIONS_FILE: &str = "/tmp/brama-skarbiec/donated-subscriptions.json";
const DONATION_RECIPIENT_ENV: &str = "SKARBIEC_DONATION_RECIPIENT";
const DEFAULT_DONATION_RECIPIENT: &str = "brama-service";

static OAUTH_REFRESH_LOCK: LazyLock<tokio::sync::Mutex<()>> =
    LazyLock::new(|| tokio::sync::Mutex::new(()));

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
            agent_id, %resource, "the authority would not issue a request-sign capability"
        );
        return None;
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
/// The providers this installation can obtain a direct capability for, resolved
/// in one pass.
///
/// `provider_capability_configured` answers for one provider and pays the whole
/// cost each time: it parses the capability map out of the environment and
/// rebuilds the client, which opens and parses the workload signing key. A
/// caller holding a catalogue asks once per model, and this installation's
/// catalogue carries several thousand of them, so an authenticated `/v1/models`
/// spent longer rebuilding the same client than any client waits for a reply.
/// The work does not vary per provider, so it is done once here and the answer
/// is a set membership test.
pub fn configured_provider_capabilities() -> std::collections::HashSet<String> {
    let mut configured = std::collections::HashSet::new();
    if client().is_none() {
        return configured;
    }
    let Some(map) = capability_map(PROVIDER_CAPABILITIES_ENV) else {
        return configured;
    };
    for (provider, capability_id) in map {
        let resource = format!("provider:{}", slug(&provider));
        if CapabilityRef::provider(&capability_id, &resource).is_ok() {
            configured.insert(provider);
        }
    }
    configured
}


/// Return whether this installation can obtain a direct provider capability.
///
/// Startup validation refuses an alias whose provider has none, so this has to
/// answer the question the request path will actually ask. It used to read one
/// environment variable the launcher filled at boot; with capabilities issued
/// where they are spent, an absent entry means nothing until the broker has
/// been asked. Asking costs one capability, which is the cheapest possible
/// proof that the path works, and is far cheaper than a gateway that starts
/// and refuses every request.
pub fn provider_capability_configured(provider: &str) -> bool {
    let resource = format!("provider:{}", slug(provider));
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
    let resource = format!("provider:{}", slug(provider));
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
            provider,
            "no capability was issued for this provider at boot; asking for one now"
        );
    }
    let Some(fresh) = issue_capability(PROVIDER_PURPOSE, &resource).await else {
        warn!(
            event = "provider_credential_issue_failed",
            provider, %resource, "the authority would not issue a capability for this resource"
        );
        return None;
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
fn redeem_provider_resource(capability_id: &str, resource: &str) -> Option<Secret> {
    let binding = CapabilityRef::provider(capability_id, resource).ok()?;
    client()?.redeem(&binding).ok()
}

/// Redeem one subscription credential at the final-use boundary. Expired
/// provider OAuth grants are refreshed only inside this scoped Brama runtime,
/// used immediately, and persisted through the local entitlements router when
/// possible.
pub async fn subscription_credential(subscription_id: &str, provider: &str) -> Option<Secret> {
    let resource = format!("provider:{}:{}", slug(provider), slug(subscription_id));
    // A capability is single-use and short-lived by contract, and model
    // discovery redeems one at boot, so the id the launcher seeded is spent
    // before the first request arrives. The provider path already answers that
    // by asking for a fresh one and, failing that, reading through the
    // authority's own grant; a subscription had neither, and every call after
    // startup reported the credential as simply unavailable.
    let seeded = configured_capability(PROVIDER_CAPABILITIES_ENV, subscription_id);
    let credential = match seeded
        .as_deref()
        .and_then(|capability_id| redeem_provider_resource(capability_id, &resource))
    {
        Some(credential) => credential,
        None => match issue_capability(PROVIDER_PURPOSE, &resource).await {
            Some(fresh) => match redeem_provider_resource(&fresh, &resource) {
                Some(credential) => credential,
                None => {
                    warn!(
                        event = "subscription_credential_redeem_failed",
                        provider, %resource,
                        "a freshly issued capability did not redeem; trying the read grant"
                    );
                    credential_by_grant(&resource).await?
                }
            },
            None => {
                warn!(
                    event = "subscription_credential_issue_failed",
                    provider, %resource,
                    "the authority would not issue a capability; trying the read grant"
                );
                credential_by_grant(&resource).await?
            }
        },
    };
    if !super::oauth_refresh::needs_refresh(&credential, provider) {
        return Some(credential);
    }
    drop(credential);

    // Refresh-token rotation is single-flight. Re-reading after the lock lets a
    // concurrent caller observe the value already written to the vault, and it
    // goes through the same acquisition as above rather than a binding built
    // from an id that may since have been spent.
    let _guard = OAUTH_REFRESH_LOCK.lock().await;
    let seeded = configured_capability(PROVIDER_CAPABILITIES_ENV, subscription_id);
    let credential = match seeded
        .as_deref()
        .and_then(|capability_id| redeem_provider_resource(capability_id, &resource))
    {
        Some(credential) => credential,
        None => credential_by_grant(&resource).await?,
    };
    if !super::oauth_refresh::needs_refresh(&credential, provider) {
        return Some(credential);
    }
    let mut fresh = match super::oauth_refresh::refresh(&credential, provider).await {
        Ok(fresh) => fresh,
        Err(_) => {
            warn!(
                event = "oauth_refresh_failed",
                provider, "OAuth refresh failed; preserving the previously redeemed credential"
            );
            return Some(credential);
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
    let table: serde_json::Value = serde_json::from_str(&raw).ok()?;
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
    let (item, field) = capability_route(resource)?;
    let output = tokio::process::Command::new(entitlements_router_bin())
        .arg("get")
        .arg(&item)
        .output()
        .await
        .ok()?;
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
    let payload: serde_json::Value = serde_json::from_slice(&output.stdout).ok()?;
    let value = payload.get("fields")?.get(&field)?.as_str()?;
    Some(Secret::from_bytes(value.as_bytes().to_vec()))
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

fn configured_subscriptions(agent_id: &str) -> Option<Result<Vec<SubscriptionEntry>, ()>> {
    let encoded = std::env::var(SUBSCRIPTION_CATALOG_ENV).ok()?;
    Some(parse_subscriptions(encoded.as_bytes(), agent_id))
}

/// One vault item row from the entitlements router's bare `list` command.
#[derive(Debug, Deserialize)]
struct VaultListItem {
    id: Option<String>,
    #[serde(default)]
    deleted: bool,
}

/// Map the router's full vault listing to one agent's subscriptions. Vault
/// resource ids look like `provider:<provider>:<rest>`; the `brama-sub-<agent>-`
/// prefix applies to `<rest>` only, after the `provider:<provider>:` segment is
/// stripped. Non-deleted resources become active entries keyed by `<rest>`.
fn parse_live_subscriptions(output: &[u8], agent_id: &str) -> Result<Vec<SubscriptionEntry>, ()> {
    let prefix = subscription_prefix(agent_id);
    let items: Vec<VaultListItem> = serde_json::from_slice(output).map_err(|_| ())?;
    Ok(items
        .into_iter()
        .filter(|item| !item.deleted)
        .filter_map(|item| {
            let id = complete_field(item.id)?;
            let resource = id.strip_prefix("provider:")?;
            let (provider, rest) = resource.split_once(':')?;
            if !rest.starts_with(&prefix) {
                return None;
            }
            Some(SubscriptionEntry {
                id: rest.to_owned(),
                provider: provider.trim().to_lowercase().replace('_', "-"),
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
    // Live vault discovery wins. A failed lookup may use the trusted
    // deployment-time metadata catalog, but never a remote credential store.
    if let Ok(live) = live_subscriptions(&broker, agent_id, false).await {
        return Ok(live);
    }
    configured_subscriptions(agent_id).unwrap_or(Err(()))
}
