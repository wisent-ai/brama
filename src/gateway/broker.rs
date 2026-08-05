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

/// Redeem one subscription credential at the final-use boundary. Expired
/// provider OAuth grants are refreshed only inside this scoped Brama runtime
/// and persisted through the local entitlements router over stdin.
pub async fn subscription_credential(subscription_id: &str, provider: &str) -> Option<Secret> {
    let capability_id = configured_capability(PROVIDER_CAPABILITIES_ENV, subscription_id)?;
    let resource = format!("provider:{}:{}", slug(provider), slug(subscription_id));
    let binding = CapabilityRef::provider(&capability_id, &resource).ok()?;
    let credential = client()?.redeem(&binding).ok()?;
    if !super::oauth_refresh::needs_refresh(&credential, provider) {
        return Some(credential);
    }
    drop(credential);

    // Refresh-token rotation is single-flight. Re-redeeming after the lock
    // ensures a concurrent caller observes the value already written to vault.
    let _guard = OAUTH_REFRESH_LOCK.lock().await;
    let credential = client()?.redeem(&binding).ok()?;
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
            provider, "refreshed OAuth credential could not be persisted"
        );
        return Some(credential);
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
