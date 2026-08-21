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
use crate::core::failure::{
    self, IMPACT_CREDENTIAL_PERSIST, IMPACT_CREDENTIAL_REFRESH, POINT_CREDENTIAL_PERSIST,
    POINT_CREDENTIAL_REDEEM,
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
const DEFAULT_DONATED_SUBSCRIPTIONS_FILE: &str = "/tmp/brama-skarbiec/donated-subscriptions.json";

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

/// One subscription account the vault holds that no agent tag points at.
///
/// Deliberately not a [`SubscriptionEntry`]: these are precisely the accounts
/// the per-agent listing cannot produce, and giving them the routable type
/// would invite a caller to route to one.
#[derive(Debug, Clone)]
pub struct UnroutableAccount {
    /// The subscription id, as its coordinate or its `brama:id:` tag spells it.
    pub id: String,
    /// The provider whose account this is.
    pub provider: String,
    /// The vault item it lives in, so the repair has an address.
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
) -> Result<Secret, Failure> {
    // Refresh-token rotation is single-flight. Re-reading after the lock lets a
    // concurrent caller observe the value already written to the vault.
    let _guard = OAUTH_REFRESH_LOCK.lock().await;
    let credential = redeem_subscription_credential(subscription_id, provider)
        .await
        .ok_or_else(|| {
            failure::envelope(
                POINT_CREDENTIAL_REDEEM,
                failure::code_for("credential_unauthorized"),
                IMPACT_CREDENTIAL_REFRESH,
                "no capability or read grant produced this subscription's credential",
            )
            .with_context("subscription", subscription_id)
            .with_context("provider", provider)
        })?;
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
            // The refusal is returned rather than discarded: the dispatcher
            // hangs it under the request it is about to refuse, which is the
            // only way the provider's own sentence reaches the caller.
            return preserve_on_failure.then_some(credential).ok_or(refused);
        }
    };
    // The reason matters more than the fact. A refreshed grant that cannot be
    // written is used once and lost, so the stale one returns on the next
    // start and the subscription reads as dead -- while this line said only
    // that something went wrong. The default recipient being a key no keyring
    // holds looked identical to a broken vault for a full day.
    if let Err(error) = put_subscription_credential(subscription_id, provider, &fresh).await {
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
            "the refreshed grant could not be persisted; the stored refresh token is stale",
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
/// provider OAuth grants are refreshed only inside this scoped Brama runtime,
/// used immediately, and persisted through the local entitlements router when
/// possible.
pub async fn subscription_credential(subscription_id: &str, provider: &str) -> Option<Secret> {
    let credential = redeem_subscription_credential(subscription_id, provider).await?;
    if !super::oauth_refresh::needs_refresh(&credential, provider) {
        return Some(credential);
    }
    drop(credential);
    refresh_subscription_credential_inner(subscription_id, provider, false, true)
        .await
        .ok()
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
    refresh_subscription_credential_inner(subscription_id, provider, true, false).await
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
    let Some(credential) = redeem_subscription_credential(subscription_id, provider).await else {
        return RefreshAhead::Unavailable(
            failure::envelope(
                POINT_CREDENTIAL_REDEEM,
                failure::code_for("credential_unauthorized"),
                IMPACT_CREDENTIAL_REFRESH,
                "no capability or read grant produced this subscription's credential",
            )
            .with_context("subscription", subscription_id)
            .with_context("provider", provider),
        );
    };
    let expires_at_ms = super::oauth_refresh::access_token_expiry_ms(&credential, provider);
    if !super::oauth_refresh::expires_within(&credential, provider, skew) {
        return RefreshAhead::NotDue { expires_at_ms };
    }
    // Dropped before the refresh so this token is not held in memory across the
    // wait for the rotation lock and the provider's answer.
    drop(credential);
    match refresh_subscription_credential_inner(subscription_id, provider, true, false).await {
        Ok(refreshed) => RefreshAhead::Refreshed {
            expires_at_ms: super::oauth_refresh::access_token_expiry_ms(&refreshed, provider),
        },
        Err(refused) => RefreshAhead::Refused(refused),
    }
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

/// Every active subscription this deployment holds, whichever agent owns it.
///
/// Metadata only, exactly as the per-agent listing: an id, a provider and a
/// status. Using any credential behind these still requires redeeming its own
/// capability, so widening the listing widens no access.
pub async fn list_all_subscriptions() -> Vec<SubscriptionEntry> {
    let broker = entitlements_router_bin();
    let Ok(output) = tokio::process::Command::new(&broker)
        .arg("list")
        .output()
        .await
    else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    parse_live_subscriptions_any_agent(&output.stdout).unwrap_or_default()
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
    let broker = entitlements_router_bin();
    let Ok(output) = tokio::process::Command::new(&broker)
        .arg("list")
        .output()
        .await
    else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    parse_unroutable_accounts(&output.stdout).unwrap_or_default()
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

async fn put_credential(item_id: &str, secret: &[u8]) -> Result<(), String> {
    use std::process::Stdio;
    use tokio::io::AsyncWriteExt;

    // `credential-put --recipient` was the vault's interface before the
    // credential lifecycle was rebuilt; the CLI answers `unknown command` to it
    // now, so every donation failed at the last step and no refreshed
    // subscription could ever land. `set-json` is the current write, and it
    // takes the document on stdin -- plaintext still crosses only the pipe.
    let document = serde_json::json!({
        "kind": "bundle",
        "schema": "skarbiec.item.v2",
        "context": {"source_kind": "donation"},
        "fields": {"value": String::from_utf8_lossy(secret)},
    })
    .to_string();
    let mut child = tokio::process::Command::new(entitlements_router_bin())
        .arg("set-json")
        .arg(item_id)
        .arg("--type")
        .arg("bundle")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("spawn entitlements router: {error}"))?;
    let write_result = match child.stdin.take() {
        Some(mut stdin) => stdin.write_all(document.as_bytes()).await,
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
        return Err(format!("credential write failed: {detail}"));
    }
    Ok(())
}

/// Why a donated credential was not banked.
///
/// The two are different repairs and different answers to the caller: a
/// document that carries no credential is the donor's to fix and left the vault
/// untouched, while a failed write is this installation's.
pub enum DonationRefusal {
    /// The document carries no credential the request path could present.
    Unusable(String),
    /// The vault write itself failed.
    Unwritable(String),
}

impl DonationRefusal {
    pub fn detail(&self) -> &str {
        match self {
            Self::Unusable(detail) | Self::Unwritable(detail) => detail,
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
    provider: &str,
    subscription_id: &str,
    api_key: &str,
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
    put_credential(&item_id, api_key.as_bytes())
        .await
        .map_err(DonationRefusal::Unwritable)?;
    // A stored credential is the sign-in that a `needs_reauthorization` waits
    // for, and nothing else clears that state. Without this the console would
    // keep demanding a re-authorization that an operator has already done, and
    // the recorded cause would go on describing a grant that is no longer in
    // the vault.
    crate::subscription_dispatch::usage::record_credential_signed_in(subscription_id, provider);
    Ok(())
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
                provider: normalized_provider(subscription_tag_value(
                    &item.tags,
                    "brama:provider:",
                )?),
                status: "active".to_owned(),
            })
        })
        .collect())
}

/// The same mapping without the owning-agent filter, for the console listing.
/// One subscription tag is the whole test: an item that carries it is a
/// subscription no matter which agent's tag sits beside it.
fn parse_live_subscriptions_any_agent(output: &[u8]) -> Result<Vec<SubscriptionEntry>, ()> {
    let items: Vec<VaultListItem> = serde_json::from_slice(output).map_err(|_| ())?;
    Ok(items
        .into_iter()
        .filter(|item| !item.deleted)
        .filter(|item| item.tags.iter().any(|tag| tag == "brama:subscription"))
        .filter_map(|item| {
            Some(SubscriptionEntry {
                id: subscription_tag_value(&item.tags, "brama:id:")?.to_owned(),
                provider: normalized_provider(subscription_tag_value(
                    &item.tags,
                    "brama:provider:",
                )?),
                status: "active".to_owned(),
            })
        })
        .collect())
}

/// Normalize a provider name the way both live parsers above do, so one
/// account cannot be `claude_code` on one listing and `claude-code` on another.
fn normalized_provider(value: &str) -> String {
    value.trim().to_lowercase().replace('_', "-")
}

/// The subscription id and provider a bare coordinate stands for.
///
/// [`subscription_resource`] writes `provider:<provider>:<subscription>`, and
/// the authority's routes table reads it back, so the coordinate identifies an
/// account even when nothing else about the item does. A two-part id is a
/// direct provider credential rather than a subscription and is not one of
/// these.
fn subscription_coordinate(item_id: &str) -> Option<(String, String)> {
    let mut parts = item_id.splitn(3, ':');
    let ("provider", Some(provider), Some(subscription)) =
        (parts.next()?, parts.next(), parts.next())
    else {
        return None;
    };
    let provider = normalized_provider(provider);
    let subscription = subscription.trim();
    (!provider.is_empty() && !subscription.is_empty()).then(|| (provider, subscription.to_owned()))
}

/// Map the router's full vault listing to the accounts no agent tag reaches.
///
/// The tag loss comes in two depths and both hide an account the same way. An
/// item that still carries `brama:subscription` and `brama:provider:` names
/// itself and is missing only the agent it belongs to; an item stripped bare --
/// `provider:kimi:brama-sub-wisent-app-kimi-primary` sat at revision 144 with
/// no tags at all while its credential kept working -- is recognised by the
/// coordinate it lives at. An item carrying any `brama:agent:` tag is routable
/// and is not reported here, whichever agent that tag names.
fn parse_unroutable_accounts(output: &[u8]) -> Result<Vec<UnroutableAccount>, ()> {
    let items: Vec<VaultListItem> = serde_json::from_slice(output).map_err(|_| ())?;
    let mut accounts: Vec<UnroutableAccount> = items
        .into_iter()
        .filter(|item| !item.deleted)
        .filter(|item| subscription_tag_value(&item.tags, "brama:agent:").is_none())
        .filter_map(|item| {
            let tagged = subscription_tag_value(&item.tags, "brama:id:").and_then(|id| {
                subscription_tag_value(&item.tags, "brama:provider:")
                    .map(|provider| (normalized_provider(provider), id.to_owned()))
            });
            let (provider, id) = tagged.or_else(|| subscription_coordinate(&item.id))?;
            Some(UnroutableAccount {
                id,
                provider,
                item: item.id,
            })
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
) -> Result<Vec<SubscriptionEntry>, ()> {
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
