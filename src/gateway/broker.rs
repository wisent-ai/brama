//! Credential seams for Brama.
//!
//! Capability redemption through the local Skarbiec broker is authoritative
//! for managed installations. A standalone desktop installation may instead
//! install an in-memory provider credential map before the server starts.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{LazyLock, Mutex, RwLock};
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::{json, Value};
use tracing::warn;
use zeroize::{Zeroize, Zeroizing};

use crate::capability::{CapabilityClient, CapabilityRef, Secret};
use crate::core::failure::{
    self, IMPACT_CREDENTIAL_PERSIST, POINT_CREDENTIAL_PERSIST, POINT_CREDENTIAL_REDEEM,
};
use wisent_errors::{Code, Failure};

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
const ENTITLEMENTS_ROUTER_TIMEOUT: Duration = Duration::from_secs(15);

type LocalCredential = Zeroizing<Vec<u8>>;
type LocalCredentialMap = HashMap<String, LocalCredential>;

static OAUTH_REFRESH_LOCK: LazyLock<tokio::sync::Mutex<()>> =
    LazyLock::new(|| tokio::sync::Mutex::new(()));
static LOCAL_PROVIDER_CREDENTIALS: LazyLock<RwLock<Option<LocalCredentialMap>>> =
    LazyLock::new(|| RwLock::new(None));
static LOCAL_SUBSCRIPTION_CREDENTIALS: LazyLock<RwLock<LocalCredentialMap>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

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
    let mut installed = LOCAL_PROVIDER_CREDENTIALS
        .write()
        .map_err(|_| "local provider credential lock is poisoned".to_owned())?;
    if installed.is_some() {
        return Err("local provider credentials were already installed".to_owned());
    }
    *installed = Some(credentials);
    Ok(())
}

pub fn local_provider_credentials_enabled() -> bool {
    LOCAL_PROVIDER_CREDENTIALS
        .read()
        .is_ok_and(|credentials| credentials.is_some())
}

pub fn local_provider_names() -> Result<Vec<String>, String> {
    let credentials = LOCAL_PROVIDER_CREDENTIALS
        .read()
        .map_err(|_| "local provider credential lock is poisoned".to_owned())?;
    let mut names = credentials
        .as_ref()
        .ok_or_else(|| "standalone credential store is not enabled".to_owned())?
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    names.sort();
    Ok(names)
}

pub fn put_local_provider_credential(provider: &str, credential: &str) -> Result<(), String> {
    let provider = provider.trim();
    if provider.is_empty() || credential.is_empty() || credential.chars().count() > 8000 {
        return Err("provider and credential must contain valid values".to_owned());
    }
    let mut credentials = LOCAL_PROVIDER_CREDENTIALS
        .write()
        .map_err(|_| "local provider credential lock is poisoned".to_owned())?;
    let credentials = credentials
        .as_mut()
        .ok_or_else(|| "standalone credential store is not enabled".to_owned())?;
    credentials.insert(
        provider.to_owned(),
        Zeroizing::new(credential.as_bytes().to_vec()),
    );
    Ok(())
}

pub fn remove_local_provider_credential(provider: &str) -> Result<bool, String> {
    let mut credentials = LOCAL_PROVIDER_CREDENTIALS
        .write()
        .map_err(|_| "local provider credential lock is poisoned".to_owned())?;
    let credentials = credentials
        .as_mut()
        .ok_or_else(|| "standalone credential store is not enabled".to_owned())?;
    Ok(credentials.remove(provider).is_some())
}

fn local_provider_credential(provider: &str) -> Option<Secret> {
    LOCAL_PROVIDER_CREDENTIALS
        .read()
        .ok()?
        .as_ref()?
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
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub login_item: Option<String>,
}

/// One Brama credential whose metadata is not complete enough to route.
///
/// Deliberately not a [`SubscriptionEntry`]: these are precisely the accounts
/// the per-agent listing cannot produce, and giving them the routable type
/// would invite a caller to route to one. Provider and id come only from tags;
/// item names stay opaque.
#[derive(Debug, Clone)]
pub struct UnroutableAccount {
    /// The subscription id from `brama:id:` tag, or None when that tag is absent.
    pub id: Option<String>,
    /// The provider from `brama:provider:` tag, or None when that tag is absent.
    pub provider: Option<String>,
    /// The Weles vault row from `brama:login:`, when an earlier write kept it.
    pub login_item: Option<String>,
    /// The vault item id it lives in, so diagnostics have an address.
    pub item: String,
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
    #[serde(default)]
    label: Option<String>,
    #[serde(default)]
    login_item: Option<String>,
}

fn configured_subscription_ids() -> std::collections::HashSet<String> {
    let Some(catalog) = std::env::var(SUBSCRIPTION_CATALOG_ENV)
        .ok()
        .and_then(|encoded| serde_json::from_str::<BrokerItems>(&encoded).ok())
    else {
        return std::collections::HashSet::new();
    };
    catalog
        .items
        .into_iter()
        .filter_map(|entry| entry.id)
        .collect()
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
/// [`provider_credential`], which falls back to the exact field-scoped route
/// when capability issuance or redemption is unavailable.
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
        match redeem_provider_resource(&capability_id, &resource) {
            Ok(secret) => return Some(secret),
            Err(refused) => prior = Some(refused),
        }
    }
    match issue_capability(PROVIDER_PURPOSE, &resource).await {
        Ok(fresh) => match redeem_provider_resource(&fresh, &resource) {
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

fn redeem_provider_resource(capability_id: &str, resource: &str) -> Result<Secret, Failure> {
    let binding = CapabilityRef::provider(capability_id, resource).map_err(|error| {
        credential_failure(
            format!("capability does not bind to `{resource}`: {error}"),
            resource,
            Code::Config,
        )
    })?;
    let broker = client().ok_or_else(|| {
        credential_failure(
            "no capability broker client: SKARBIEC_CAP_SOCKET, SKARBIEC_WORKLOAD_ID, or the workload signing key is missing or unreadable",
            resource,
            Code::Config,
        )
    })?;
    broker.redeem(&binding).map_err(|error| {
        credential_failure(
            format!("authority refused capability redemption: {error}"),
            resource,
            failure::code_for("credential_unauthorized"),
        )
    })
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
        match redeem_provider_resource(&capability_id, &resource) {
            Ok(secret) => return Ok(secret),
            Err(refused) => prior = Some(refused),
        }
    }
    match issue_capability(PROVIDER_PURPOSE, &resource).await {
        Ok(fresh) => match redeem_provider_resource(&fresh, &resource) {
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

async fn refresh_subscription_credential_inner(
    subscription_id: &str,
    provider: &str,
    force: bool,
) -> Result<Secret, Failure> {
    // Refresh-token rotation is single-flight. Re-reading after the lock lets a
    // concurrent caller observe the value already written to the vault.
    let _guard = OAUTH_REFRESH_LOCK.lock().await;
    let credential = redeem_subscription_credential(subscription_id, provider).await?;
    if !force && !super::oauth_refresh::needs_refresh(&credential, provider) {
        return Ok(credential);
    }
    let mut fresh = match super::oauth_refresh::refresh(&credential, provider).await {
        Ok(fresh) => fresh,
        Err(refused) => {
            let refused = refused
                .with_context("subscription", subscription_id)
                .with_context("provider", provider);
            warn!(
                event = "oauth_refresh_failed",
                provider,
                error = refused.detail.as_deref().unwrap_or_default(),
                envelope = %refused.to_json(),
                "OAuth refresh failed"
            );
            record_refusal(subscription_id, provider, &refused);
            return Err(refused);
        }
    };
    // The reason matters more than the fact. A refreshed grant that cannot be
    // written is used once and lost, so the stale one returns on the next
    // start and the subscription reads as dead -- while this line said only
    // that something went wrong. The default recipient being a key no keyring
    // holds looked identical to a broken vault for a full day.
    if let Err(error) = put_subscription_credential(subscription_id, provider, &fresh).await {
        let persist_detail = error.clone();
        let unpersisted = failure::envelope(
            POINT_CREDENTIAL_PERSIST,
            Code::Config,
            IMPACT_CREDENTIAL_PERSIST,
            error,
        )
        .with_context("subscription", subscription_id)
        .with_context("provider", provider);
        warn!(
            event = "oauth_refresh_persist_failed",
            provider,
            error = unpersisted.detail.as_deref().unwrap_or_default(),
            envelope = %unpersisted.to_json(),
            "refreshed OAuth credential could not be persisted; the rotated grant is lost \
             and the stored one is already dead at the provider"
        );
        // Using it once and moving on is what turned working accounts into
        // permanent `invalid_grant`: the provider rotated the refresh token, the
        // new one was never written, and the vault kept a grant the provider had
        // already invalidated. So this is a failed refresh rather than a
        // successful one with a warning attached: the grant is dropped here
        // instead of being spent from memory, and the subscription is recorded
        // as needing a re-authorization so the renewal path runs.
        crate::subscription_dispatch::usage::record_reauthorization_needed(
            subscription_id,
            provider,
            &persist_detail,
        );
        return Err(unpersisted);
    }
    let refreshed = Secret::from_bytes(std::mem::take(&mut *fresh));
    // What the vault now holds, said in the ledger rather than left to be
    // rediscovered: a reader asking why a token died early needs the instant it
    // was rotated and the instant it expires, and neither is in the vault.
    let expires_at_ms = super::oauth_refresh::access_token_expiry_ms(&refreshed, provider);
    crate::subscription_dispatch::usage::record_credential_active(
        subscription_id,
        provider,
        expires_at_ms,
        true,
    );
    Ok(refreshed)
}

/// Put a refused refresh in the ledger when the provider disowned the grant,
/// and leave the record alone when nothing about the grant was learned.
///
/// Both refresh paths classify here -- the sweep ahead of expiry and the forced
/// refresh a rejected request triggers -- so a credential cannot read as dead on
/// one path and healthy on the other.
fn record_refusal(subscription_id: &str, provider: &str, refused: &Failure) {
    // The provider's own sentence, or a stand-in when it refused without one:
    // the ledger's cause is what an operator reads, and an empty cause is a row
    // that says a sign-in is needed without saying why.
    let detail = refused
        .detail
        .as_deref()
        .unwrap_or("the provider refused this credential's refresh without saying why");
    match super::oauth_refresh::classify_refusal(refused) {
        super::oauth_refresh::RefreshRefusal::Definitive => {
            warn!(
                event = "credential_refresh_refused_definitively",
                subscription = %subscription_id,
                provider,
                error = detail,
                "the provider will not accept this grant again; only a sign-in repairs it"
            );
            crate::subscription_dispatch::usage::record_reauthorization_needed(
                subscription_id,
                provider,
                detail,
            );
        }
        super::oauth_refresh::RefreshRefusal::Transient => {
            warn!(
                event = "credential_refresh_transient_skipped",
                subscription = %subscription_id,
                provider,
                error = detail,
                "the refresh failed without the provider disowning the grant; the \
                 credential is left as it stands for the next sweep"
            );
        }
    }
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

/// Force one OAuth refresh after the provider rejects a grant whose local
/// expiry still claims it is valid. The rejected grant is not returned when
/// refresh fails because retrying it would only repeat the provider error; the
/// refusal itself is, so the caller can report what the provider said instead
/// of reporting that something happened.
pub async fn refresh_subscription_credential(
    subscription_id: &str,
    provider: &str,
) -> Result<Secret, Failure> {
    refresh_subscription_credential_inner(subscription_id, provider, true).await
}

/// Whether this provider's subscription credentials are OAuth grants that can be
/// refreshed, rather than API keys that never expire.
pub fn supports_oauth_refresh(provider: &str) -> bool {
    super::oauth_refresh::supports_refresh(provider)
}

/// What one refresh-ahead attempt concluded, for a caller that never holds the
/// credential itself.
pub enum RefreshAhead {
    /// This grant has more than the skew window left, or the provider is not one
    /// whose credentials Brama refreshes at all. `expires_at_ms` is present only
    /// when the credential states an expiry.
    NotDue { expires_at_ms: Option<i64> },
    /// The grant was refreshed and the new one is in the vault.
    Refreshed { expires_at_ms: Option<i64> },
    /// The refresh was refused, or the refreshed grant could not be stored.
    /// The ledger already carries the verdict; this is the sentence to log.
    Refused(Failure),
    /// No capability and no read grant produced a credential to look at, so
    /// nothing is known about the grant behind it.
    Unavailable(Failure),
}

/// Replace one subscription's access token before it expires, when it expires
/// inside `skew`.
///
/// The credential is read twice on the path that does refresh, and that is not
/// an oversight: the expiry has to be read before deciding, and the refresh
/// below deliberately re-reads under the rotation lock so a caller that waited
/// for a concurrent refresh observes what that refresh wrote instead of
/// rotating a grant the provider has already invalidated. Only a credential
/// that is genuinely due pays for the second read.
pub async fn refresh_subscription_credential_ahead(
    subscription_id: &str,
    provider: &str,
    skew: Duration,
) -> RefreshAhead {
    let credential = match redeem_subscription_credential(subscription_id, provider).await {
        Ok(credential) => credential,
        Err(refused) => return RefreshAhead::Unavailable(refused),
    };
    let expires_at_ms = super::oauth_refresh::access_token_expiry_ms(&credential, provider);
    if !super::oauth_refresh::expires_within(&credential, provider, skew) {
        return RefreshAhead::NotDue { expires_at_ms };
    }
    // Dropped before the refresh so this token is not held in memory across the
    // wait for the rotation lock and the provider's answer.
    drop(credential);
    match refresh_subscription_credential_inner(subscription_id, provider, true).await {
        Ok(refreshed) => RefreshAhead::Refreshed {
            expires_at_ms: super::oauth_refresh::access_token_expiry_ms(&refreshed, provider),
        },
        Err(refused) => RefreshAhead::Refused(refused),
    }
}

/// Enumerate one agent's subscription metadata through the configured
/// acquisition boundary and preserve whether that boundary answered.
///
/// Onboarding needs to distinguish an empty account from an unavailable
/// Skarbiec/entitlements route; flattening both to an empty vector would present
/// a dependency failure as a valid zero-state import.
///
/// This always performs live discovery. The trusted startup catalog belongs to
/// internal routing only and cannot turn a failed user-facing read into success.
pub async fn discover_subscriptions(agent_id: &str) -> Result<Vec<SubscriptionEntry>, String> {
    let broker = entitlements_router_bin();
    let mut entries = live_subscriptions(&broker, agent_id, true).await?;
    for donated in donated_subscriptions(agent_id)? {
        match entries.iter_mut().find(|entry| entry.id == donated.id) {
            Some(existing) => *existing = donated,
            None => entries.push(donated),
        }
    }
    Ok(entries)
}

/// Enumerate one agent's subscription metadata for internal routing.
///
/// A trusted deployment catalog may keep routing available when live discovery
/// fails, but the live failure is always logged and is never used by
/// [`discover_subscriptions`].
pub async fn list_subscriptions(agent_id: &str) -> Vec<SubscriptionEntry> {
    let mut entries = match list_subscriptions_result(agent_id).await {
        Ok(entries) => entries,
        Err(error) => {
            warn!(
                event = "subscription_routing_discovery_failed",
                agent_id,
                %error,
                "live subscription discovery failed and no trusted catalog was usable"
            );
            Vec::new()
        }
    };
    match donated_subscriptions(agent_id) {
        Ok(donated) => {
            for entry in donated {
                match entries.iter_mut().find(|existing| existing.id == entry.id) {
                    Some(existing) => *existing = entry,
                    None => entries.push(entry),
                }
            }
        }
        Err(error) => warn!(
            event = "donated_subscription_overlay_failed",
            agent_id,
            %error,
            "routing continues without the donated-subscriptions overlay"
        ),
    }
    entries
}

/// Every active subscription this deployment holds, whichever agent owns it.
pub async fn list_all_subscriptions() -> Result<Vec<SubscriptionEntry>, String> {
    let output = router_output("list all subscriptions", |command| {
        command.arg("list");
    })
    .await?;
    if !output.status.success() {
        return Err(router_refusal("list all subscriptions", &output));
    }
    parse_live_subscriptions_any_agent(&output.stdout)
}

/// Every subscription account in the vault that carries no `brama:agent:` tag.
///
/// Discovery finds an account by that tag, so an item that loses it stops
/// existing for every caller and every screen while its credential stays
/// perfectly valid. It is the one failure this deployment had no way to see:
/// the account is not expired, not refused and not retired -- it is simply not
/// looked at, and nothing that lists subscriptions can say so, because the
/// listing is the thing that lost it.
///
/// Read through the same `list` the per-agent discovery already shells, so
/// nothing here becomes a second reader of the vault, and metadata only: an
/// item id, a provider and a subscription id, never a value.
pub async fn list_unroutable_accounts() -> Vec<UnroutableAccount> {
    let output = match router_output("list unroutable subscription accounts", |command| {
        command.arg("list");
    })
    .await
    {
        Ok(output) => output,
        Err(error) => {
            warn!(event = "unroutable_account_listing_failed", %error);
            return Vec::new();
        }
    };
    if !output.status.success() {
        warn!(
            event = "unroutable_account_listing_failed",
            error = %router_refusal("list unroutable subscription accounts", &output)
        );
        return Vec::new();
    }
    match parse_unroutable_accounts(&output.stdout) {
        Ok(accounts) => accounts,
        Err(error) => {
            warn!(event = "unroutable_account_listing_failed", %error);
            Vec::new()
        }
    }
}

/// Incomplete Brama credential metadata that still identifies an exact
/// subscription. These entries are renewal candidates, not routable entries:
/// Weles must prove the declared primary maps to the same subscription id, and
/// the resulting donation adds the missing routing tags before any agent sees
/// it.
pub async fn list_recoverable_subscriptions() -> Vec<SubscriptionEntry> {
    list_unroutable_accounts()
        .await
        .into_iter()
        .filter_map(|account| {
            Some(SubscriptionEntry {
                id: account.id?,
                provider: account.provider?,
                status: "active".to_owned(),
                label: None,
                login_item: account.login_item,
            })
        })
        .collect()
}

/// Path of the donated-subscriptions overlay file.
pub fn donated_subscriptions_path() -> PathBuf {
    std::env::var(DONATED_SUBSCRIPTIONS_FILE_ENV)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
            PathBuf::from(home)
                .join(".stado")
                .join("var")
                .join("brama")
                .join("donated-subscriptions.json")
        })
}

/// Overlay entries for one agent. A missing file is an empty overlay; every
/// other read or decode failure remains visible to the caller.
fn donated_subscriptions(agent_id: &str) -> Result<Vec<SubscriptionEntry>, String> {
    let path = donated_subscriptions_path();
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(format!(
                "read donated subscriptions file `{}`: {error}",
                path.to_string_lossy()
            ));
        }
    };
    parse_subscriptions(text.as_bytes(), agent_id).map_err(|error| {
        format!(
            "decode donated subscriptions file `{}`: {error}",
            path.to_string_lossy()
        )
    })
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
pub fn donated_add(
    agent_id: &str,
    id: &str,
    provider: &str,
    label: Option<&str>,
    login_item: Option<&str>,
) -> Result<(), String> {
    update_donated_items(|items| {
        items.retain(|item| item.get("id").and_then(Value::as_str) != Some(id));
        items.push(json!({
            "id": id,
            "provider": provider,
            "agent_id": agent_id,
            "status": "active",
            "label": label,
            "login_item": login_item,
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

async fn bounded_output(
    binary: &str,
    operation: &str,
    configure: impl FnOnce(&mut tokio::process::Command),
) -> Result<std::process::Output, String> {
    let mut command = tokio::process::Command::new(binary);
    command.kill_on_drop(true);
    configure(&mut command);
    match tokio::time::timeout(ENTITLEMENTS_ROUTER_TIMEOUT, command.output()).await {
        Ok(Ok(output)) => Ok(output),
        Ok(Err(error)) => Err(format!("{operation}: {error}")),
        Err(_) => Err(format!(
            "{operation} timed out after {} seconds; the child was killed",
            ENTITLEMENTS_ROUTER_TIMEOUT.as_secs()
        )),
    }
}

async fn router_output(
    operation: &str,
    configure: impl FnOnce(&mut tokio::process::Command),
) -> Result<std::process::Output, String> {
    bounded_output(&entitlements_router_bin(), operation, configure).await
}

fn router_refusal(operation: &str, output: &std::process::Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let detail = stderr.trim();
    if detail.is_empty() {
        format!(
            "{operation} failed with status {}; stderr was empty",
            output.status
        )
    } else {
        format!("{operation} failed with status {}: {detail}", output.status)
    }
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
fn capability_refusal_detail(stdout: &[u8], stderr: &[u8]) -> String {
    let stderr = String::from_utf8_lossy(stderr);
    let stderr = stderr.trim();
    if !stderr.is_empty() {
        return stderr.to_owned();
    }
    let Ok(document) = serde_json::from_slice::<Value>(stdout) else {
        return "authority refused the capability without a reason".to_owned();
    };
    let reason = document
        .get("reason")
        .and_then(Value::as_str)
        .unwrap_or("authority refused the capability");
    match document.get("remedy").and_then(Value::as_str) {
        Some(remedy) if !remedy.is_empty() => format!("{reason}; {remedy}"),
        _ => reason.to_owned(),
    }
}

fn issued_capability_id(output: &std::process::Output, resource: &str) -> Result<String, Failure> {
    if !output.status.success() {
        let detail = capability_refusal_detail(&output.stdout, &output.stderr);
        return Err(credential_failure(
            format!(
                "capability issuance for `{resource}` failed with status {}: {detail}",
                output.status
            ),
            resource,
            failure::code_for("credential_unauthorized"),
        ));
    }
    let document: Value = serde_json::from_slice(&output.stdout).map_err(|error| {
        credential_failure(
            format!("capability issuance returned malformed JSON: {error}"),
            resource,
            Code::Unknown,
        )
    })?;
    document
        .get("capability_id")
        .and_then(Value::as_str)
        .filter(|id| !id.trim().is_empty())
        .map(str::to_owned)
        .ok_or_else(|| {
            credential_failure(
                "capability issuance response is missing non-empty field `capability_id`",
                resource,
                Code::Unknown,
            )
        })
}

/// Obtain one capability on the request path.
async fn issue_capability(purpose: &str, resource: &str) -> Result<String, Failure> {
    let arguments = issue_arguments(purpose, resource);
    let output = router_output("issue capability", |command| {
        command.args(arguments);
    })
    .await
    .map_err(|error| credential_failure(error, resource, Code::Unknown))?;
    issued_capability_id(&output, resource)
}

fn issue_capability_blocking(purpose: &str, resource: &str) -> Result<String, Failure> {
    use std::process::Stdio;

    let mut child = std::process::Command::new(entitlements_router_bin())
        .args(issue_arguments(purpose, resource))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| {
            credential_failure(
                format!("issue capability: {error}"),
                resource,
                Code::Unknown,
            )
        })?;
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => {
                let output = child.wait_with_output().map_err(|error| {
                    credential_failure(
                        format!("collect capability issuance output: {error}"),
                        resource,
                        Code::Unknown,
                    )
                })?;
                return issued_capability_id(&output, resource);
            }
            Ok(None) if started.elapsed() < ENTITLEMENTS_ROUTER_TIMEOUT => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Ok(None) => {
                let kill_error = child.kill().err();
                let output = child.wait_with_output().map_err(|error| {
                    credential_failure(
                        format!(
                            "capability issuance timed out after {} seconds; kill result: {}; collect output: {error}",
                            ENTITLEMENTS_ROUTER_TIMEOUT.as_secs(),
                            kill_error
                                .as_ref()
                                .map(ToString::to_string)
                                .unwrap_or_else(|| "child killed".to_owned())
                        ),
                        resource,
                        Code::Unknown,
                    )
                })?;
                let stderr = String::from_utf8_lossy(&output.stderr);
                return Err(credential_failure(
                    format!(
                        "capability issuance timed out after {} seconds; {}; stderr: {}",
                        ENTITLEMENTS_ROUTER_TIMEOUT.as_secs(),
                        kill_error
                            .map(|error| format!("kill failed: {error}"))
                            .unwrap_or_else(|| "the child was killed".to_owned()),
                        if stderr.trim().is_empty() {
                            "<empty>"
                        } else {
                            stderr.trim()
                        }
                    ),
                    resource,
                    Code::Unknown,
                ));
            }
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(credential_failure(
                    format!("poll capability issuance child: {error}"),
                    resource,
                    Code::Unknown,
                ));
            }
        }
    }
}

/// The vault coordinate a resource stands for, as the operator wrote it.
///
/// The same table the authority consults, read here so nothing in this process
/// ever decides for itself which credential a purpose means.
fn capability_route(resource: &str) -> Result<(String, String), Failure> {
    let path = std::env::var_os("SKARBIEC_CAPABILITY_ROUTES_FILE").ok_or_else(|| {
        credential_failure(
            "SKARBIEC_CAPABILITY_ROUTES_FILE is not configured",
            resource,
            Code::Config,
        )
    })?;
    let raw = std::fs::read_to_string(&path).map_err(|error| {
        credential_failure(
            format!(
                "read capability routes file `{}`: {error}",
                path.to_string_lossy()
            ),
            resource,
            Code::Config,
        )
    })?;
    let document: Value = serde_json::from_str(&raw).map_err(|error| {
        credential_failure(
            format!(
                "capability routes file `{}` contains malformed JSON: {error}",
                path.to_string_lossy()
            ),
            resource,
            Code::Config,
        )
    })?;
    let table = document.get("routes").unwrap_or(&document);
    let entry = table.get(resource).ok_or_else(|| {
        credential_failure(
            format!("no capability route maps resource `{resource}`"),
            resource,
            Code::Config,
        )
    })?;
    let item = entry
        .get("item")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            credential_failure(
                format!("capability route for `{resource}` is missing non-empty field `item`"),
                resource,
                Code::Config,
            )
        })?
        .to_owned();
    let field = entry
        .get("field")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            credential_failure(
                format!("capability route for `{resource}` is missing non-empty field `field`"),
                resource,
                Code::Config,
            )
        })?
        .to_owned();
    Ok((item, field))
}

/// Read one provider credential through the grant the vault already carries.
///
/// Redeeming a capability is the stronger path and stays first. It is not the
/// only one the fleet provisions: some providers are granted as a plain
/// per-field read to a named consumer.
async fn credential_by_grant(resource: &str) -> Result<Secret, Failure> {
    let (item, field) = capability_route(resource)?;
    let output = router_output("read credential through grant", |command| {
        command.arg("get").arg(&item);
    })
    .await
    .map_err(|error| {
        credential_failure(error, resource, Code::Unknown)
            .with_context("item", item.as_str())
            .with_context("field", field.as_str())
    })?;
    if !output.status.success() {
        return Err(credential_failure(
            router_refusal("read credential through grant", &output),
            resource,
            failure::code_for("credential_unauthorized"),
        )
        .with_context("item", item.as_str())
        .with_context("field", field.as_str()));
    }
    let payload: Value = serde_json::from_slice(&output.stdout).map_err(|error| {
        credential_failure(
            format!("credential read for item `{item}` returned malformed JSON: {error}"),
            resource,
            Code::Unknown,
        )
        .with_context("item", item.as_str())
        .with_context("field", field.as_str())
    })?;
    let fields = payload
        .get("fields")
        .and_then(Value::as_object)
        .ok_or_else(|| {
            credential_failure(
                format!("credential document for item `{item}` is missing object field `fields`"),
                resource,
                Code::Unknown,
            )
            .with_context("item", item.as_str())
            .with_context("field", field.as_str())
        })?;
    let value = fields.get(&field).ok_or_else(|| {
        credential_failure(
            format!("credential document for item `{item}` is missing field `{field}`"),
            resource,
            Code::Unknown,
        )
        .with_context("item", item.as_str())
        .with_context("field", field.as_str())
    })?;
    let bytes = match value {
        Value::String(value) => value.as_bytes().to_vec(),
        Value::Null => {
            return Err(credential_failure(
                format!("credential document for item `{item}` has null field `{field}`"),
                resource,
                Code::Unknown,
            )
            .with_context("item", item.as_str())
            .with_context("field", field.as_str()));
        }
        value => serde_json::to_vec(value).map_err(|error| {
            credential_failure(
                format!(
                    "credential field `{field}` for item `{item}` could not be preserved as JSON: {error}"
                ),
                resource,
                Code::Unknown,
            )
            .with_context("item", item.as_str())
            .with_context("field", field.as_str())
        })?,
    };
    Ok(Secret::from_bytes(bytes))
}

async fn put_credential(
    item_id: &str,
    secret: &[u8],
    tags: Option<&[String]>,
) -> Result<(), String> {
    use std::process::Stdio;
    use tokio::io::AsyncWriteExt;

    let document = serde_json::json!({
        "kind": "bundle",
        "schema": "skarbiec.item.v2",
        "context": {"source_kind": "donation"},
        "fields": {"value": String::from_utf8_lossy(secret)},
    })
    .to_string();
    let mut command = tokio::process::Command::new(entitlements_router_bin());
    command
        .kill_on_drop(true)
        .arg("set-json")
        .arg(item_id)
        .arg("--type")
        .arg("bundle");
    if let Some(tags) = tags {
        command.arg("--tags").arg(tags.join(","));
    }
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("spawn credential write: {error}"))?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| "credential write child stdin is unavailable".to_owned())?;
    match tokio::time::timeout(
        ENTITLEMENTS_ROUTER_TIMEOUT,
        stdin.write_all(document.as_bytes()),
    )
    .await
    {
        Ok(Ok(())) => {}
        Ok(Err(error)) => {
            let _ = child.kill().await;
            return Err(format!("write credential document to child stdin: {error}"));
        }
        Err(_) => {
            let _ = child.kill().await;
            return Err(format!(
                "write credential document timed out after {} seconds; the child was killed",
                ENTITLEMENTS_ROUTER_TIMEOUT.as_secs()
            ));
        }
    }
    drop(stdin);
    let output =
        match tokio::time::timeout(ENTITLEMENTS_ROUTER_TIMEOUT, child.wait_with_output()).await {
            Ok(Ok(output)) => output,
            Ok(Err(error)) => return Err(format!("wait for credential write child: {error}")),
            Err(_) => {
                return Err(format!(
                    "credential write timed out after {} seconds; the child was killed",
                    ENTITLEMENTS_ROUTER_TIMEOUT.as_secs()
                ));
            }
        };
    if !output.status.success() {
        return Err(router_refusal("credential write", &output));
    }
    Ok(())
}

fn put_local_subscription_credential(item_id: &str, secret: &[u8]) -> Result<(), String> {
    LOCAL_SUBSCRIPTION_CREDENTIALS
        .write()
        .map_err(|_| "local subscription credential lock is poisoned".to_owned())?
        .insert(item_id.to_owned(), Zeroizing::new(secret.to_vec()));
    Ok(())
}

/// The tags a subscription credential write must store, given what the item
/// already carries.
///
/// Discovery finds an account by `brama:subscription` plus
/// `brama:agent:<agent>` (see `parse_live_subscriptions`), so an item missing
/// either is not a degraded account: it does not exist for any caller, while
/// its credential stays perfectly valid and every check that counts
/// credentials keeps answering green.
///
/// This is the writer's half of that contract, and it exists because the write
/// path had no such half. `put_subscription_credential` passed `None` for
/// tags, which means `skarbiec set-json` keeps whatever the item already had
/// and a fresh item is created with nothing -- so the rotation path could mint
/// a subscription that no agent could ever route to, and did.
///
/// Measured on charless-mac-mini on 2026-09-02: three of the four subscription
/// accounts in that vault -- `brama-sub-wisent-app-codex-secondary`,
/// `...-claude-primary`, `...-kimi-primary` -- carried `brama:provider:` and
/// `brama:id:` and neither `brama:subscription` nor any `brama:agent:`. Every
/// agent on that host could reach exactly one credential, so the single block
/// on it took the documentation gate of every repository down. One of the three
/// redeemed on the first probe after its tags were restored: a working paid
/// credential had been invisible the whole time. The same shape had already
/// cost this fleet a day through a missing `brama:agent:weles` tag.
///
/// The structural three are derived, never asked for: the provider and the
/// subscription id are what this write is for, and the mark follows from being
/// a subscription at all. The agent binding is the one thing that cannot be
/// derived -- it is an entitlement decision about who may spend a paid plan --
/// so a write that would leave an item with no agent tag is refused here
/// rather than completed with a guess. A refusal at write time is the only
/// place that requirement is met by whoever is doing the writing.
pub fn subscription_tags_for_write(
    existing: &[String],
    provider: &str,
    subscription_id: &str,
) -> Result<Vec<String>, String> {
    let mut tags: Vec<String> = existing.to_vec();
    if !tags.iter().any(|tag| tag == "brama:subscription") {
        tags.push("brama:subscription".to_owned());
    }
    for (prefix, wanted) in [
        ("brama:provider:", normalized_provider(provider)),
        ("brama:id:", subscription_id.to_owned()),
    ] {
        let declared = tags
            .iter()
            .filter_map(|tag| tag.strip_prefix(prefix))
            .filter(|value| !value.is_empty())
            .collect::<Vec<_>>();
        let disagrees = declared.iter().any(|value| {
            if prefix == "brama:provider:" {
                normalized_provider(value) != wanted
            } else {
                *value != wanted.as_str()
            }
        });
        if disagrees {
            return Err(format!(
                "the vault item already carries {prefix}{}; refusing to write {prefix}{wanted} over it",
                declared.join(",")
            ));
        }
        if declared.is_empty() {
            tags.push(format!("{prefix}{wanted}"));
        }
    }
    if !tags.iter().any(|tag| {
        tag.strip_prefix("brama:agent:")
            .is_some_and(|a| !a.is_empty())
    }) {
        return Err(format!(
            "writing this credential would store subscription {subscription_id} for provider \
             {provider} with no 'brama:agent:<agent>' tag, so discovery could not see it and no \
             agent could route to it while its credential stayed valid. Which agents may spend a \
             paid plan is an entitlement decision this write cannot derive: tag the item with \
             `stado host retag-vault-item <host> provider:{provider}:{subscription_id} --tags …` \
             and repeat the write"
        ));
    }
    Ok(tags)
}
async fn donated_credential_tags(
    item_id: &str,
    agent_id: &str,
    provider: &str,
    subscription_id: &str,
    login_item: Option<&str>,
) -> Result<Vec<String>, DonationRefusal> {
    let output = router_output("list vault tags for donated credential", |command| {
        command.arg("list");
    })
    .await
    .map_err(DonationRefusal::Unwritable)?;
    if !output.status.success() {
        return Err(DonationRefusal::Unwritable(router_refusal(
            "list vault tags for donated credential",
            &output,
        )));
    }
    let items: Vec<VaultListItem> = serde_json::from_slice(&output.stdout).map_err(|error| {
        DonationRefusal::Unwritable(format!("decode vault tags for donated credential: {error}"))
    })?;
    let mut tags = items
        .into_iter()
        .find(|item| item.id == item_id)
        .map(|item| item.tags)
        .unwrap_or_default();

    let agent_tag = format!("brama:agent:{agent_id}");
    if !tags.contains(&agent_tag) {
        if tags.iter().any(|tag| {
            tag.strip_prefix("brama:agent:")
                .is_some_and(|agent| !agent.is_empty())
        }) {
            return Err(DonationRefusal::MappingConflict(format!(
                "{item_id} is not assigned to agent {agent_id}; refusing to replace its credential"
            )));
        }
        tags.push(agent_tag);
    }
    // Agent tags are additive entitlements, unlike the provider, subscription
    // and login identity. Renewing a shared credential must preserve every
    // existing consumer rather than reject the other authorized agents.
    let mut tags = subscription_tags_for_write(&tags, provider, subscription_id)
        .map_err(DonationRefusal::MappingConflict)?;
    if let Some(login_item) = login_item {
        let declared = tags
            .iter()
            .filter_map(|tag| tag.strip_prefix("brama:login:"))
            .filter(|value| !value.is_empty())
            .collect::<Vec<_>>();
        if declared.iter().any(|value| *value != login_item) {
            return Err(DonationRefusal::MappingConflict(format!(
                "{item_id} is mapped to login {}; refusing credential minted by {login_item}",
                declared.join(",")
            )));
        }
        if declared.is_empty() {
            tags.push(format!("brama:login:{login_item}"));
        }
    }
    Ok(tags)
}

/// Why a donated credential was not banked.
///
/// The two are different repairs and different answers to the caller: a
/// document that carries no credential is the donor's to fix and left the vault
/// untouched, while a failed write is this installation's.
pub enum DonationRefusal {
    /// The document carries no credential the request path could present.
    Unusable(String),
    /// Existing subscription metadata names another account.
    MappingConflict(String),
    /// The vault write itself failed.
    Unwritable(String),
}

impl DonationRefusal {
    pub fn detail(&self) -> &str {
        match self {
            Self::Unusable(detail) | Self::MappingConflict(detail) | Self::Unwritable(detail) => {
                detail
            }
        }
    }
}

/// Store one donated OAuth credential blob through the local entitlements
/// router; plaintext crosses only the child process stdin pipe.
///
/// The item is the one the operator's routes table already names for this
/// subscription. Minting a fresh id per donation produced a credential nothing
/// could read: reads resolve through that table, and a new id has no entry in
/// it, so the donation was inert by construction.
///
/// A donation is refused unless the document reduces to a bearer, because this
/// write lands on the one coordinate a provider's `-primary` subscription is
/// read from and there is no second copy. On 2026-08-19 the vault item
/// `provider:codex:brama-sub-wisent-app-codex-primary` -- an account with 11,123
/// recorded requests -- held a browser context options document
/// (`deviceScaleFactor`, `extraHTTPHeaders`, `recordHar`, `recordVideo`,
/// `viewport`) at revision 318, so every call routed to it was refused by the
/// provider-authentication check while the ledger read `active`: the sign-in
/// this records had marked it so. The only length bound the boundary applied was
/// 1..8000 characters, which a re-authentication trajectory's own configuration
/// object satisfies. The predicate is the request path's own reduction, so
/// nothing a request could have presented is refused here.
pub async fn put_donated_credential(
    agent_id: &str,
    provider: &str,
    subscription_id: &str,
    api_key: &str,
    login_item: Option<&str>,
) -> Result<(), DonationRefusal> {
    let item_id = format!("provider:{provider}:{subscription_id}");
    // The bearer is derived only to prove one can be, and dropped unread.
    if let Err(detail) = crate::providers::adapter::credential_key(&item_id, api_key) {
        warn!(
            event = "donated_credential_unusable",
            provider,
            subscription = subscription_id,
            %detail,
            "a donated document carries no credential; the stored one is left as it is"
        );
        return Err(DonationRefusal::Unusable(detail));
    }
    if local_provider_credentials_enabled() {
        put_local_subscription_credential(&item_id, api_key.as_bytes())
            .map_err(DonationRefusal::Unwritable)?;
    } else {
        let tags =
            donated_credential_tags(&item_id, agent_id, provider, subscription_id, login_item)
                .await?;
        put_credential(&item_id, api_key.as_bytes(), Some(&tags))
            .await
            .map_err(DonationRefusal::Unwritable)?;
    }
    crate::subscription_dispatch::usage::record_credential_signed_in(subscription_id, provider);
    Ok(())
}

pub fn remove_donated_credential(provider: &str, subscription_id: &str) -> Result<(), String> {
    if !local_provider_credentials_enabled() {
        return Ok(());
    }
    LOCAL_SUBSCRIPTION_CREDENTIALS
        .write()
        .map_err(|_| "local subscription credential lock is poisoned".to_owned())?
        .remove(&subscription_resource(provider, subscription_id));
    Ok(())
}

/// Store a rotated subscription credential, with the tags discovery requires.
///
/// This used to pass `None` for tags, which leaves whatever the item already
/// carried and gives a fresh item nothing at all. See
/// [`subscription_tags_for_write`] for what that cost.
async fn put_subscription_credential(
    subscription_id: &str,
    provider: &str,
    credential: &[u8],
) -> Result<(), String> {
    let item_id = format!("provider:{}:{}", slug(provider), slug(subscription_id));
    if local_provider_credentials_enabled() {
        return put_local_subscription_credential(&item_id, credential);
    }
    let existing = existing_item_tags(&item_id).await?;
    let tags = subscription_tags_for_write(&existing, provider, subscription_id)?;
    put_credential(&item_id, credential, Some(&tags)).await
}

/// The tags one vault item carries right now, empty when it does not exist.
async fn existing_item_tags(item_id: &str) -> Result<Vec<String>, String> {
    let output = router_output("list vault tags for credential write", |command| {
        command.arg("list");
    })
    .await?;
    if !output.status.success() {
        return Err(router_refusal(
            "list vault tags for credential write",
            &output,
        ));
    }
    let items: Vec<VaultListItem> = serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("decode vault tags for credential write: {error}"))?;
    Ok(items
        .into_iter()
        .find(|item| item.id == item_id)
        .map(|item| item.tags)
        .unwrap_or_default())
}

fn complete_field(value: Option<String>) -> Option<String> {
    value.filter(|field| !field.is_empty() && field.trim() == field)
}

fn required_broker_field(
    value: Option<String>,
    field: &str,
    index: usize,
) -> Result<String, String> {
    complete_field(value)
        .ok_or_else(|| format!("subscription catalog row {index} is missing valid field `{field}`"))
}

fn parse_subscriptions(output: &[u8], agent_id: &str) -> Result<Vec<SubscriptionEntry>, String> {
    let response: BrokerItems = serde_json::from_slice(output)
        .map_err(|error| format!("subscription catalog contains malformed JSON: {error}"))?;
    let mut entries = Vec::new();
    for (index, entry) in response.items.into_iter().enumerate() {
        let entry_agent_id = required_broker_field(entry.agent_id, "agent_id", index)?;
        if entry_agent_id != agent_id {
            continue;
        }
        entries.push(SubscriptionEntry {
            id: required_broker_field(entry.id, "id", index)?,
            provider: required_broker_field(entry.provider, "provider", index)?,
            status: required_broker_field(entry.status, "status", index)?,
            label: complete_field(entry.label),
            login_item: complete_field(entry.login_item),
        });
    }
    Ok(entries)
}

fn configured_subscriptions(agent_id: &str) -> Option<Result<Vec<SubscriptionEntry>, String>> {
    let encoded = std::env::var(SUBSCRIPTION_CATALOG_ENV).ok()?;
    Some(parse_subscriptions(encoded.as_bytes(), agent_id))
}

/// One vault item row from the entitlements router's bare `list` command.
#[derive(Debug, Deserialize)]
struct VaultListItem {
    /// The coordinate the item lives at. Discovery reads tags, not ids -- but
    /// an item that has lost every tag still has this, and it is the only
    /// thing left to recognise a subscription account by.
    #[serde(default)]
    id: String,
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
fn live_subscription_entry(item: &VaultListItem) -> Result<SubscriptionEntry, String> {
    let coordinate = if item.id.trim().is_empty() {
        "<unnamed vault item>"
    } else {
        item.id.as_str()
    };
    let id = subscription_tag_value(&item.tags, "brama:id:").ok_or_else(|| {
        format!("subscription vault item `{coordinate}` is missing tag `brama:id:<id>`")
    })?;
    let provider = subscription_tag_value(&item.tags, "brama:provider:").ok_or_else(|| {
        format!("subscription vault item `{coordinate}` is missing tag `brama:provider:<provider>`")
    })?;
    Ok(SubscriptionEntry {
        id: id.to_owned(),
        provider: normalized_provider(provider),
        status: "active".to_owned(),
        label: None,
        login_item: subscription_tag_value(&item.tags, "brama:login:").map(str::to_owned),
    })
}

fn parse_live_subscriptions(
    output: &[u8],
    agent_id: &str,
) -> Result<Vec<SubscriptionEntry>, String> {
    let agent_tag = format!("brama:agent:{agent_id}");
    let items: Vec<VaultListItem> = serde_json::from_slice(output)
        .map_err(|error| format!("subscription listing returned malformed JSON: {error}"))?;
    let mut entries = Vec::new();
    for item in items.into_iter().filter(|item| {
        !item.deleted
            && item.tags.iter().any(|tag| tag == "brama:subscription")
            && item.tags.iter().any(|tag| tag == &agent_tag)
    }) {
        entries.push(live_subscription_entry(&item)?);
    }
    Ok(entries)
}

/// The same mapping without the owning-agent filter, for the console listing.
fn parse_live_subscriptions_any_agent(output: &[u8]) -> Result<Vec<SubscriptionEntry>, String> {
    let items: Vec<VaultListItem> = serde_json::from_slice(output)
        .map_err(|error| format!("subscription listing returned malformed JSON: {error}"))?;
    let mut entries = Vec::new();
    for item in items
        .into_iter()
        .filter(|item| !item.deleted && item.tags.iter().any(|tag| tag == "brama:subscription"))
    {
        entries.push(live_subscription_entry(&item)?);
    }
    Ok(entries)
}

/// Normalize a provider name the way both live parsers above do, so one
/// account cannot be `claude_code` on one listing and `claude-code` on another.
fn normalized_provider(value: &str) -> String {
    value.trim().to_lowercase().replace('_', "-")
}

/// Map the router's listing to Brama credentials that cannot be routed.
///
/// A complete subscription needs the marker and at least one agent tag. An
/// older credential write could preserve `brama:id:` and `brama:provider:` but
/// drop either routing tag; those items are included so automatic renewal can
/// restore their metadata. Items with no Brama id remain unrelated vault data.
fn parse_unroutable_accounts(output: &[u8]) -> Result<Vec<UnroutableAccount>, String> {
    let items: Vec<VaultListItem> = serde_json::from_slice(output)
        .map_err(|error| format!("vault account listing returned malformed JSON: {error}"))?;
    let mut accounts: Vec<UnroutableAccount> = items
        .into_iter()
        .filter(|item| !item.deleted)
        .filter(|item| subscription_tag_value(&item.tags, "brama:id:").is_some())
        .filter(|item| subscription_tag_value(&item.tags, "brama:provider:").is_some())
        .filter(|item| {
            !item.tags.iter().any(|tag| tag == "brama:subscription")
                || subscription_tag_value(&item.tags, "brama:agent:").is_none()
        })
        .map(|item| {
            let id = subscription_tag_value(&item.tags, "brama:id:").map(str::to_owned);
            let provider =
                subscription_tag_value(&item.tags, "brama:provider:").map(normalized_provider);
            let login_item = subscription_tag_value(&item.tags, "brama:login:").map(str::to_owned);
            UnroutableAccount {
                id,
                provider,
                login_item,
                item: item.id,
            }
        })
        .collect();
    accounts.sort_by(|left, right| left.item.cmp(&right.item));
    Ok(accounts)
}

type LiveSubscriptionsCache = Mutex<HashMap<String, (Instant, Vec<SubscriptionEntry>)>>;

/// Live discovery results per agent. Entries are stored only after a
/// successful listing so a failed shell never poisons the cache.
static LIVE_SUBSCRIPTIONS_CACHE: LazyLock<LiveSubscriptionsCache> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

const LIVE_SUBSCRIPTIONS_CACHE_TTL: Duration = Duration::from_secs(60);

/// Resolve one agent's subscriptions from the vault, serving a fresh cached
/// listing unless `bypass_cache` is set (used when a lookup failed and the
/// caller wants to re-check the vault instead of trusting a stale entry).
async fn live_subscriptions(
    broker: &str,
    agent_id: &str,
    bypass_cache: bool,
) -> Result<Vec<SubscriptionEntry>, String> {
    if !bypass_cache {
        if let Ok(cache) = LIVE_SUBSCRIPTIONS_CACHE.lock() {
            if let Some((fetched_at, entries)) = cache.get(agent_id) {
                if fetched_at.elapsed() < LIVE_SUBSCRIPTIONS_CACHE_TTL {
                    return Ok(entries.clone());
                }
            }
        }
    }
    let entries = list_subscriptions_live(broker, agent_id).await?;
    if let Ok(mut cache) = LIVE_SUBSCRIPTIONS_CACHE.lock() {
        cache.insert(agent_id.to_owned(), (Instant::now(), entries.clone()));
    }
    Ok(entries)
}

/// Shell the entitlements router's bare `list`, which returns a JSON array of
/// every vault item (`{"id","type","tags","updated_at","deleted","versions"}`).
async fn list_subscriptions_live(
    broker: &str,
    agent_id: &str,
) -> Result<Vec<SubscriptionEntry>, String> {
    let output = bounded_output(broker, "list subscriptions", |command| {
        command.arg("list");
    })
    .await?;
    if !output.status.success() {
        return Err(router_refusal("list subscriptions", &output));
    }
    parse_live_subscriptions(&output.stdout, agent_id)
}

async fn list_subscriptions_result(agent_id: &str) -> Result<Vec<SubscriptionEntry>, String> {
    let broker = entitlements_router_bin();
    match live_subscriptions(&broker, agent_id, false).await {
        Ok(mut live) => {
            match configured_subscriptions(agent_id) {
                Some(Ok(configured)) => {
                    for entry in configured {
                        if !live.iter().any(|existing| existing.id == entry.id) {
                            live.push(entry);
                        }
                    }
                }
                Some(Err(error)) => warn!(
                    event = "subscription_catalog_invalid",
                    agent_id,
                    %error,
                    "live subscriptions remain usable without the invalid trusted catalog"
                ),
                None => {}
            }
            Ok(live)
        }
        Err(live_error) => {
            warn!(
                event = "subscription_live_discovery_failed",
                agent_id,
                error = %live_error,
                "internal routing will use the trusted catalog if one is available"
            );
            match configured_subscriptions(agent_id) {
                Some(Ok(configured)) => Ok(configured),
                Some(Err(catalog_error)) => Err(format!(
                    "{live_error}; trusted subscription catalog is invalid: {catalog_error}"
                )),
                None => Err(live_error),
            }
        }
    }
}
