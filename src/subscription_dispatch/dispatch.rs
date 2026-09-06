//! Route canonical `provider/model` requests through stateless provider APIs.

use axum::http::HeaderMap;
use serde_json::Value;
use std::collections::HashMap;
use std::io::Read;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};
use tracing::{info, warn};

use crate::core::failure::{
    self, IMPACT_MODEL_REQUEST, POINT_BOUNDED_ROTATION, POINT_CREDENTIAL_SELECTION,
    POINT_MODEL_SELECTION,
};
use crate::crypto;
use crate::gateway::broker;
use crate::providers::adapter as provider_registry;
use crate::subscription_dispatch::usage;
use crate::types::{ModelRequest, ModelResponse};
use wisent_errors::Failure;

/// The envelope for one refused model request: where it broke, what the caller
/// loses, the reason verbatim, and the failure underneath it when there is one.
fn refusal_envelope(
    model: &str,
    point: &str,
    kind: &str,
    message: &str,
    cause: Option<Failure>,
) -> Failure {
    let refusal = failure::envelope(
        point,
        // The kind is the caller's own answer, passed in rather than assumed:
        // an envelope that says `rate_limit` beside a `503 credential_
        // unauthorized` body sends the operator looking for a busy provider
        // while the actual break is an authorization chain. The code is looked
        // up from that kind, so the two readings cannot drift apart.
        failure::code_for(kind),
        IMPACT_MODEL_REQUEST,
        message,
    )
    .with_context("model", model);
    match cause {
        Some(cause) => refusal.caused_by(cause),
        None => refusal,
    }
}

/// Refuse one model request, saying where it broke and what the layer below
/// said, and hand the caller the sentence it has always been handed.
///
/// The message is the envelope's detail and nothing else: clients parse these
/// strings, so the envelope travels in the log beside them, never inside them.
fn refuse(
    request: &ModelRequest,
    point: &str,
    message: String,
    cause: Option<Failure>,
) -> ModelResponse {
    refuse_as(request, point, "subscription_unavailable", message, cause)
}

/// The same refusal, naming the kind the HTTP edge will answer with.
fn refuse_as(
    request: &ModelRequest,
    point: &str,
    kind: &str,
    message: String,
    cause: Option<Failure>,
) -> ModelResponse {
    let refusal = refusal_envelope(&request.model, point, kind, &message, cause);
    warn!(
        event = "dispatch_refused",
        model = %request.model,
        envelope = %refusal.to_json(),
        "{}",
        refusal.render()
    );
    ModelResponse::failure(&request.model, message)
}

fn failure_detail(failure: &Failure) -> String {
    failure.detail.clone().unwrap_or_else(|| failure.render())
}

fn remember_failure(slot: &mut Option<Failure>, failure: Failure) {
    let prior = slot.take();
    *slot = Some(match prior {
        Some(prior) => failure.caused_by(prior),
        None => failure,
    });
}

/// Which kind an emptied credential pool is, so the log envelope and the HTTP
/// answer say the same thing.
///
/// A provider that refused every credential and a vault that produced none are
/// both authorization failures no wait repairs; only a genuinely exhausted pool
/// is capacity.
///
/// A credential inside an authorization block counts with the first group. The
/// router skips a blocked credential without calling the provider, so a
/// credential the provider had already refused looked, for the half hour its
/// block lasted, exactly like one that was merely out of quota - and the caller
/// was told to retry. That is the same defect as reporting a refused redemption
/// as capacity, arriving one layer further in.
fn rotation_failure_kind(cause: PoolEmptyCause) -> &'static str {
    if cause.needs_authorization() {
        "credential_unauthorized"
    } else {
        "subscription_unavailable"
    }
}

/// Why an emptied credential pool emptied, as one value.
///
/// The buffered and streaming paths reach this decision independently and used
/// to spell it twice. They now share it, because the two spellings drifting is
/// how the same broken credential comes to answer `503` to one caller and `429`
/// to another.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PoolEmptyCause {
    /// A provider refused a credential outright during this request.
    pub auth_rejection: bool,
    /// A credential was skipped because its recorded block is an authorization
    /// block: the same refusal, still being served from the ledger.
    pub reauthorization_block: bool,
    /// The vault produced no credential at all -- no capability, no grant.
    pub unredeemable_credential: bool,
}

impl PoolEmptyCause {
    /// Whether repairing this needs an authorization, not a wait.
    pub fn needs_authorization(self) -> bool {
        self.auth_rejection || self.reauthorization_block || self.unredeemable_credential
    }
}

/// The sentence one emptied pool is reported with.
///
/// Four causes, and the caller acts on each differently: a provider that
/// refused needs a sign-in, a credential inside an authorization block is that
/// same refusal still recorded, a vault that produced nothing needs a
/// capability or grant repaired, and everything else is quota worth waiting
/// out. Only the last is retryable.
///
/// The block case is why this is a named function with a test beside it.
/// `codex` answered `401 Your session has ended. Please log in again`, which
/// recorded `needs_reauthorization` and a half-hour block; every request inside
/// that window skipped the credential without asking anyone, emptied the pool
/// with nothing observed, and fell through to the capacity sentence. The ledger
/// had recorded the authorization failure the whole time, and the caller was
/// told to retry -- which is the defect ARCHITECTURE.md records as fixed,
/// reappearing one layer further in.
pub fn pool_empty_summary(provider: &str, cause: PoolEmptyCause) -> String {
    if cause.auth_rejection || cause.reauthorization_block {
        auth_rejected_summary(provider)
    } else if cause.unredeemable_credential {
        unredeemable_credential_summary(provider)
    } else {
        bounded_unavailable_summary(provider)
    }
}

/// The request path's own sentence for a pool whose credentials the vault
/// would not produce.
///
/// These three sentences are what an operator reads when a call is refused, and
/// a readiness check that invented its own wording for the same fault would
/// make one broken chain look like two. They are written once here and used by
/// the buffered path, the streaming path, and [`probe_subscription_redemption`].
fn unredeemable_credential_summary(provider: &str) -> String {
    format!(
        "no '{provider}' credential could be redeemed for agent; a capability, \
         read grant, or this installation's trust material is missing"
    )
}

/// The request path's own sentence for a pool every one of whose credentials is
/// inside a recorded rate-limit block.
fn bounded_unavailable_summary(provider: &str) -> String {
    format!("all bounded '{provider}' credentials unavailable for agent")
}

/// The request path's own sentence for a pool every one of whose credentials
/// the provider itself refused.
fn auth_rejected_summary(provider: &str) -> String {
    format!(
        "all bounded '{provider}' credentials were rejected by the provider; \
         re-authorization required"
    )
}

/// The request path's own sentence for an agent with no active credential of
/// this provider at all -- the answer a subscription whose vault item lost its
/// `brama:agent:` tag produces, because discovery can no longer see it.
pub(crate) fn no_active_credential_summary(provider: &str) -> String {
    format!("no active '{provider}' credential for agent")
}

/// Perform, for one subscription, the act a health check cannot infer: redeem
/// its credential and answer in the request path's vocabulary.
///
/// Readiness used to ask whether a subscription contributed a model, which is a
/// declaration -- the catalogue answers it from discovery, and discovery
/// answered `true` all through the morning of 2026-08-18 while every live call
/// was refused. This asks the question a request asks, at the same boundary,
/// through the same broker call, and returns what refused it.
///
/// Redeeming is not the whole question, so the document is reduced the way a
/// request reduces it before this answers -- see
/// [`redeemed_credential_verdict`]. The key that reduction yields is dropped
/// unread: the caller learns that a request could have presented something,
/// and never what.
pub async fn probe_subscription_redemption(
    subscription_id: &str,
    provider: &str,
) -> Result<(), String> {
    if crate::journal::is_retired(subscription_id) {
        return Err(no_active_credential_summary(provider));
    }
    // Checked before redeeming, in the request path's own order: a blocked
    // credential is skipped there without a provider call, so a readiness probe
    // that redeemed it anyway would report a working credential the router
    // refuses to use. An authorization block is named as one, for the same
    // reason the request path names it: `/readyz` is the deploy check, and
    // "wait" and "sign in again" are different instructions to whoever reads it.
    if usage::is_blocked(subscription_id) {
        return Err(if usage::needs_reauthorization(subscription_id) {
            auth_rejected_summary(provider)
        } else {
            bounded_unavailable_summary(provider)
        });
    }
    match broker::subscription_credential(subscription_id, provider).await {
        Ok(credential) => {
            let item = broker::subscription_resource(provider, subscription_id);
            let verdict = match credential.expose_utf8() {
                Ok(secret) => redeemed_credential_verdict(&item, secret),
                Err(error) => Err(format!(
                    "Skarbiec item `{item}` holds bytes that are not valid UTF-8: {error}"
                )),
            };
            drop(credential);
            verdict
        }
        Err(refused) => {
            warn!(
                event = "subscription_redemption_probe_failed",
                subscription = subscription_id,
                provider,
                envelope = %refused.to_json(),
                "{}",
                refused.render()
            );
            Err(failure_detail(&refused))
        }
    }
}

/// Whether a redeemed document is a credential a request could actually
/// present, in the words the router itself would use.
///
/// `Some` from the broker is not the same statement as "the credential
/// redeemed", and reading it as one is how `/readyz` came to answer
/// `redeemable: true` for `brama-sub-wisent-app-codex-secondary` on 2026-09-02
/// while the same gateway's own model call answered `no value at
/// provider:codex:brama-sub-wisent-app-codex-secondary#value` and then `OAuth
/// credential has no refresh token`. The vault row is account metadata: the
/// capability redemption is refused, the read grant hands back the document
/// anyway, it carries no access token, and nothing between there and the
/// provider looked. The catalogue path already said so in its own report --
/// "holds a JSON object with fields [...], which carries no credential" -- so
/// readiness was the only reader being reassured.
///
/// This is the request path's own reduction, not a second opinion: exactly the
/// [`provider_registry::credential_key`] every dispatch calls before it builds
/// an authorization header. The key it returns is dropped unread; the refusal
/// it returns names the item and the field names it looked for and never any
/// credential material.
pub(crate) fn redeemed_credential_verdict(item: &str, secret: &str) -> Result<(), String> {
    provider_registry::credential_key(item, secret).map(drop)
}

fn max_selector_models() -> usize {
    "3".parse().expect("valid selector model limit")
}

fn max_credential_attempts() -> usize {
    "2".parse().expect("valid credential attempt limit")
}
const MODEL_CACHE_TTL: Duration = Duration::from_secs(300);
fn is_permanent_auth_failure(error: &str) -> bool {
    let error = error.to_ascii_lowercase();
    error.contains("refresh token was revoked")
        || error.contains("access token could not be refreshed")
        || error.contains("invalid_grant")
}

fn is_auth_failure(error: &str) -> bool {
    let error = error.to_ascii_lowercase();
    error.contains("invalid authentication")
        || error.contains("authentication_error")
        || error.contains("failed to authenticate")
        || error.contains("401")
        || error.contains("provider_authentication")
        || error.contains("oauth")
        || is_permanent_auth_failure(&error)
}

/// Retire one credential the provider has permanently refused, and say so in the
/// ledger.
///
/// The journal is what stops this credential being selected again; the ledger
/// record is what lets a reader see that it was retired, when, and why. Without
/// the second one a retired subscription and a subscription nobody has used all
/// month render as the same row.
async fn mark_credential_revoked(credential_id: &str, provider: &str, cause: &str) {
    crate::journal::retire(credential_id);
    usage::record_credential_disabled(credential_id, provider, cause);
}

struct CachedRegistryModels {
    fetched: Instant,
    models: Vec<provider_registry::RegistryModel>,
}

static REGISTRY_MODEL_CACHE: LazyLock<Mutex<HashMap<String, CachedRegistryModels>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Discovery failures are cached briefly too: without it every catalog call
/// re-pays full provider timeouts for credentials that are stale anyway.
const MODEL_FAILURE_CACHE_TTL: Duration = Duration::from_secs(60);
static REGISTRY_MODEL_FAILURE_CACHE: LazyLock<Mutex<HashMap<String, (Instant, String)>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Why model discovery last refused one subscription, when it did.
///
/// [`discover_subscription_models`] records every per-subscription refusal and
/// then throws the list away unless the pool ended up with no models at all —
/// so one working subscription silences the reason every other one failed, and
/// `/readyz` could say only "active subscription, no model discovered".
///
/// Measured on charless-mac-mini on 2026-09-02: claude-code and kimi both
/// redeemed and both discovered nothing, while codex on the same host in the
/// same sweep discovered five. Establishing why took reading this module's
/// branches and counting `static_models` entries, because no surface would say
/// it. Two of the three explanations that reading had to separate — a
/// catalogue with nothing configured for the provider, and a discovery path
/// that never ran — are ours to fix, and the third is not; the sentence this
/// exposes names which.
pub fn discovery_failure(provider: &str, subscription_id: &str) -> Option<String> {
    let key = format!("{}:{subscription_id}", provider.trim());
    REGISTRY_MODEL_FAILURE_CACHE
        .lock()
        .ok()?
        .get(&key)
        .filter(|(fetched, _)| fetched.elapsed() < MODEL_FAILURE_CACHE_TTL)
        .map(|(_, error)| error.clone())
}

pub fn is_subscription_model(model: &str) -> bool {
    provider_for(model).is_some()
}

pub fn provider_requires_caller_identity(model: &str) -> bool {
    matches!(provider_for(model), Some("claude-code" | "codex" | "kimi"))
}

pub(crate) fn provider_for(model: &str) -> Option<&str> {
    provider_registry::provider_id_from_route(model)
}

pub async fn registry_models_for_agent(
    agent_id: &str,
) -> Result<Vec<provider_registry::RegistryModel>, String> {
    let entries = broker::list_subscriptions(agent_id)
        .await
        .into_iter()
        .filter(|entry| entry.status == "active" && !crate::journal::is_retired(&entry.id))
        .collect::<Vec<_>>();
    discover_subscription_models(entries).await
}

/// Models already discovered for this deployment's subscriptions and its
/// directly-keyed providers, for the desktop console.
///
/// The console authenticates with a bearer, not an agent signature, so it has
/// no agent identity to discover against. Without this its catalogue carried
/// the public vendor list alone -- which knows `openai` but not that a `codex`
/// subscription is what pays for those models here -- so a subscription's own
/// screen could not name a single model it pays for.
///
/// This reads caches and never waits on a provider. Discovering ten providers
/// inline, several holding credentials their vendor has since revoked, took
/// longer than the console's own request deadline: the catalogue answered with
/// a timeout, which is worse than answering with the part that is known. A cold
/// cache is filled by one background pass instead, so the first read is fast
/// and thin and the next is complete.
pub async fn registry_models_for_console() -> Result<Vec<provider_registry::RegistryModel>, String>
{
    let mut models = cached_registry_models();
    spawn_console_discovery();
    models.sort_by(|left, right| left.route_id.cmp(&right.route_id));
    models.dedup_by(|left, right| left.route_id == right.route_id);
    Ok(models)
}

/// Everything currently held in the discovery cache, whatever put it there.
fn cached_registry_models() -> Vec<provider_registry::RegistryModel> {
    let Ok(cache) = REGISTRY_MODEL_CACHE.lock() else {
        return Vec::new();
    };
    cache
        .values()
        .filter(|item| item.fetched.elapsed() < MODEL_CACHE_TTL)
        .flat_map(|item| item.models.clone())
        .collect()
}

/// One background discovery pass at a time. A console that refreshes every few
/// seconds must not stack a provider sweep per click.
static CONSOLE_DISCOVERY_RUNNING: AtomicBool = AtomicBool::new(false);

fn spawn_console_discovery() {
    if CONSOLE_DISCOVERY_RUNNING.swap(true, Ordering::SeqCst) {
        return;
    }
    tokio::spawn(async move {
        let entries = match broker::list_all_subscriptions().await {
            Ok(entries) => entries
                .into_iter()
                .filter(|entry| entry.status == "active" && !crate::journal::is_retired(&entry.id))
                .collect::<Vec<_>>(),
            Err(error) => {
                tracing::warn!(
                    %error,
                    event = "console_subscription_listing_failed",
                    "subscription models could not be refreshed for the console catalogue"
                );
                Vec::new()
            }
        };
        if let Err(error) = discover_subscription_models(entries).await {
            tracing::warn!(
                %error,
                event = "console_subscription_discovery_failed",
                "no subscription published models for the console catalogue"
            );
        }
        discover_direct_provider_models().await;
        CONSOLE_DISCOVERY_RUNNING.store(false, Ordering::SeqCst);
    });
}

/// Models for the providers this gateway holds a direct credential for.
///
/// A subscription is not the only way a provider gets paid for, and the console
/// showed the consequence: `featherless` sat there marked available with a model
/// count of zero, because nothing discovered a provider unless a subscription
/// pointed at it and the public vendor list has no `featherless` in it at all.
///
/// A provider that cannot be reached contributes nothing and says so in the
/// log; it does not fail the catalogue for the others.
async fn discover_direct_provider_models() -> Vec<provider_registry::RegistryModel> {
    let mut discovered = Vec::new();
    for provider in broker::configured_provider_capabilities() {
        let cache_key = format!("{provider}:direct");
        if let Some(models) = REGISTRY_MODEL_CACHE.lock().ok().and_then(|cache| {
            cache
                .get(&cache_key)
                .filter(|item| item.fetched.elapsed() < MODEL_CACHE_TTL)
                .map(|item| item.models.clone())
        }) {
            discovered.extend(models);
            continue;
        }
        if REGISTRY_MODEL_FAILURE_CACHE
            .lock()
            .ok()
            .and_then(|cache| {
                cache
                    .get(&cache_key)
                    .filter(|(fetched, _)| fetched.elapsed() < MODEL_FAILURE_CACHE_TTL)
                    .cloned()
            })
            .is_some()
        {
            continue;
        }
        let Some(secret) = broker::provider_credential(&provider).await else {
            continue;
        };
        let Ok(secret) = secret.expose_utf8() else {
            continue;
        };
        let resource = broker::provider_resource(&provider);
        match provider_registry::discover_models(&provider, &resource, secret).await {
            Ok(models) => {
                if let Ok(mut cache) = REGISTRY_MODEL_CACHE.lock() {
                    cache.insert(
                        cache_key,
                        CachedRegistryModels {
                            fetched: Instant::now(),
                            models: models.clone(),
                        },
                    );
                }
                discovered.extend(models);
            }
            Err(error) => {
                if let Ok(mut cache) = REGISTRY_MODEL_FAILURE_CACHE.lock() {
                    cache.insert(cache_key, (Instant::now(), error.clone()));
                }
                tracing::warn!(
                    %provider,
                    %error,
                    event = "direct_provider_discovery_failed",
                    "a provider with a direct credential published no models"
                );
            }
        }
    }
    discovered
}

async fn discover_subscription_models(
    entries: Vec<broker::SubscriptionEntry>,
) -> Result<Vec<provider_registry::RegistryModel>, String> {
    if entries.is_empty() {
        return Ok(Vec::new());
    }

    let mut models_by_route = HashMap::new();
    let mut failures = Vec::new();
    for entry in entries {
        let provider = entry.provider.trim();
        let cache_key = format!("{provider}:{}", entry.id);
        let cached = REGISTRY_MODEL_CACHE.lock().ok().and_then(|cache| {
            cache
                .get(&cache_key)
                .filter(|item| item.fetched.elapsed() < MODEL_CACHE_TTL)
                .map(|item| item.models.clone())
        });
        let models = if let Some(cached) = cached {
            cached
        } else {
            let recent_failure = REGISTRY_MODEL_FAILURE_CACHE.lock().ok().and_then(|cache| {
                cache
                    .get(&cache_key)
                    .filter(|(fetched, _)| fetched.elapsed() < MODEL_FAILURE_CACHE_TTL)
                    .map(|(_, error)| error.clone())
            });
            if let Some(error) = recent_failure {
                failures.push(format!("{}: {error}", entry.id));
                continue;
            }
            let secret = match broker::subscription_credential(&entry.id, provider).await {
                Ok(secret) => secret,
                Err(refused) => {
                    let detail = failure_detail(&refused);
                    warn!(
                        event = "subscription_model_credential_failed",
                        subscription = %entry.id,
                        provider,
                        envelope = %refused.to_json(),
                        "{}",
                        refused.render()
                    );
                    failures.push(format!("{}: {detail}", entry.id));
                    continue;
                }
            };
            let secret = match secret.expose_utf8() {
                Ok(secret) => secret,
                Err(error) => {
                    failures.push(format!("{}: credential is not UTF-8: {error}", entry.id));
                    continue;
                }
            };
            let item = broker::subscription_resource(provider, &entry.id);
            let discovered = match provider_registry::discover_models(provider, &item, secret).await
            {
                Ok(models) => models,
                Err(error) => {
                    if let Ok(mut cache) = REGISTRY_MODEL_FAILURE_CACHE.lock() {
                        cache.insert(cache_key.clone(), (Instant::now(), error.clone()));
                    }
                    failures.push(format!("{}: {error}", entry.id));
                    continue;
                }
            };
            if let Ok(mut cache) = REGISTRY_MODEL_CACHE.lock() {
                cache.insert(
                    cache_key,
                    CachedRegistryModels {
                        fetched: Instant::now(),
                        models: discovered.clone(),
                    },
                );
            }
            discovered
        };
        for model in models {
            models_by_route
                .entry(model.route_id.clone())
                .or_insert(model);
        }
    }
    if models_by_route.is_empty() && !failures.is_empty() {
        return Err(format!(
            "could not discover native provider models: {}",
            failures.join("; ")
        ));
    }
    let mut models = models_by_route.into_values().collect::<Vec<_>>();
    models.sort_by(|left, right| left.route_id.cmp(&right.route_id));
    Ok(models)
}

fn provider_matches(candidate: &str, requested: &str) -> bool {
    candidate.trim().eq_ignore_ascii_case(requested)
}

fn eligible_subscription_entries(
    entries: Vec<broker::SubscriptionEntry>,
    provider: &str,
    target: Option<&crate::types::BillingTarget>,
) -> Result<Vec<broker::SubscriptionEntry>, String> {
    if let Some(target) = target {
        if target.account_id.trim().is_empty() || target.subscription_id.trim().is_empty() {
            return Err("billingTarget accountId and subscriptionId are required".into());
        }
        if !provider_matches(&target.provider_id, provider) {
            return Err(format!(
                "billingTarget provider '{}' does not match model provider '{provider}'",
                target.provider_id
            ));
        }
    }
    Ok(entries
        .into_iter()
        .filter(|entry| {
            provider_matches(&entry.provider, provider)
                && entry.status == "active"
                && !crate::journal::is_retired(&entry.id)
                && target.is_none_or(|target| entry.id == target.subscription_id)
        })
        .collect())
}

pub(crate) async fn authenticate_agent(
    headers: &HeaderMap,
    raw_body: &[u8],
) -> Result<String, String> {
    let agent_id = headers
        .get("x-agent-id")
        .and_then(|v| v.to_str().ok())
        .filter(|s| !s.is_empty())
        .map(String::from)
        .ok_or_else(|| "missing x-agent-id header".to_string())?;
    let ts = headers
        .get("x-agent-timestamp")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let sig = headers
        .get("x-agent-signature")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    let secret = broker::get_agent_auth_secret(&agent_id)
        .await
        .ok_or_else(|| "no auth secret for agent".to_string())?;

    let headers_for_check = crypto::HmacHeaders {
        agent_id: &agent_id,
        timestamp: ts,
        signature: sig,
    };
    crypto::verify_agent_hmac(&headers_for_check, raw_body, secret.expose())
        .map_err(|e| format!("auth: {e}"))?;
    Ok(agent_id)
}

fn random_u64() -> Result<u64, String> {
    let mut bytes = u64::default().to_ne_bytes();
    std::fs::File::open("/dev/urandom")
        .and_then(|mut source| source.read_exact(&mut bytes))
        .map_err(|_| "operating system randomness is unavailable".to_string())?;
    Ok(u64::from_ne_bytes(bytes))
}

/// Shuffle only inside runs whose key compares equal, preserving the caller's
/// order between runs.
///
/// Random placement exists to decorrelate accounts that would otherwise be
/// hammered in list order, not to overrule a ranking the caller computed. A
/// key derived from plan state says something true about the candidates; an
/// equal key says nothing, and "nothing" is the only thing chance should
/// decide.
fn shuffle_within_equal<T, K: PartialOrd>(
    items: &mut [T],
    key: impl Fn(&T) -> K,
) -> Result<(), String> {
    let mut start = 0;
    while start < items.len() {
        let mut end = start + 1;
        while end < items.len()
            && key(&items[end]).partial_cmp(&key(&items[start])) == Some(std::cmp::Ordering::Equal)
        {
            end += 1;
        }
        let run: &mut [T] = &mut items[start..end];
        if run.len() > 1 {
            for i in (1..run.len()).rev() {
                let j = (random_u64()? as usize) % (i + 1);
                run.swap(i, j);
            }
        }
        start = end;
    }
    Ok(())
}

/// The plan headroom of a route, as the freest usable subscription behind it.
///
/// Routing reads the ledger, never the provider: a number that costs a call to
/// learn cannot be paid for on every selection. A subscription nobody has
/// measured counts as fully available -- its first call writes the reading
/// that corrects that -- and a route with no usable subscription counts as
/// full, so it sorts behind every route that can actually serve.
fn route_plan_key(subscriptions: &[broker::SubscriptionEntry], route_id: &str) -> f64 {
    let Some(provider) = provider_for(route_id) else {
        return 1.0;
    };
    let mut best: Option<f64> = None;
    for entry in subscriptions {
        if !provider_matches(&entry.provider, provider)
            || entry.status != "active"
            || crate::journal::is_retired(&entry.id)
            || usage::is_blocked(&entry.id)
        {
            continue;
        }
        let fraction = usage::used_fraction(&entry.id).unwrap_or(0.0);
        best = Some(best.map_or(fraction, |current: f64| current.min(fraction)));
    }
    best.unwrap_or(1.0)
}

/// Order candidate models by what their plans have left, fullest window last.
///
/// Selectors used to shuffle the whole list, which spent accounts at random
/// while the ledger already held each one's own statement of how spent it
/// was. The order is now the provider's own numbers; chance only breaks ties,
/// so two accounts at the same utilization still decorrelate and no two
/// accounts at different ones ever trade places.
async fn order_models_by_plan(agent_id: &str, models: &mut [String]) -> Result<(), String> {
    let subscriptions = broker::list_subscriptions(agent_id).await;
    models.sort_by(|left, right| {
        route_plan_key(&subscriptions, left)
            .partial_cmp(&route_plan_key(&subscriptions, right))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    shuffle_within_equal(models, |model| {
        ordered_float_key(route_plan_key(&subscriptions, model))
    })
}

/// An f64 routing key as an orderable integer: sign-flipped bits, so equal
/// fractions compare equal and ties stay shuffles rather than sorts.
fn ordered_float_key(value: f64) -> i64 {
    let bits = value.to_bits() as i64;
    if bits < 0 {
        bits ^ i64::MAX
    } else {
        bits
    }
}

/// One agent's pinned credential for one provider, process-local.
///
/// A pin is the dispatcher's memory of which account served this agent most
/// recently, held so consecutive requests land on the same subscription until
/// its own plan window ends. That is what makes the provider's prompt cache
/// reachable at all -- a cache entry lives behind one account, and the old
/// shuffle scattered an agent's turns across all of them -- and it keeps one
/// turn's spend in one account's ledger instead of smearing it across the
/// pool. The pin is a preference, never a grant: it is consulted after
/// eligibility, skipped when the credential is blocked, retired, or reporting
/// a full window, and it never outlives the window it was read from. It is
/// not persisted, because its whole meaning expires with the window anyway
/// and a restart simply shuffles once.
struct Pin {
    credential_id: String,
    expires_at_ms: i64,
}

/// How long a pin survives when the credential's own windows name no reset.
///
/// Five hours matches the shortest plan window any provider on this fleet
/// publishes, so the fallback cannot outlive the thing it approximates. The
/// cap exists because a provider's reset instant is trusted only within a
/// day: past that, an error in one header would hold an agent on one account
/// for longer than any real window lasts.
const DEFAULT_PIN_MS: i64 = 5 * 60 * 60 * 1_000;
const MAX_PIN_MS: i64 = 24 * 60 * 60 * 1_000;

static PINS: LazyLock<Mutex<HashMap<(String, String), Pin>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or_default()
}

/// The credential this agent is pinned to for this provider, if the pin is
/// still inside its own window.
fn pinned_credential(agent_id: &str, provider: &str) -> Option<String> {
    let key = (agent_id.to_string(), provider.to_string());
    let mut pins = PINS.lock().ok()?;
    let pin = pins.get(&key)?;
    if pin.expires_at_ms <= now_ms() {
        pins.remove(&key);
        return None;
    }
    Some(pin.credential_id.clone())
}

/// Pin this agent to the credential that just served it, until that
/// credential's tightest window says otherwise.
fn pin_credential(agent_id: &str, provider: &str, credential_id: &str) {
    let now = now_ms();
    let expires_at_ms = usage::next_reset_ms(credential_id)
        .unwrap_or_else(|| now.saturating_add(DEFAULT_PIN_MS))
        .min(now.saturating_add(MAX_PIN_MS));
    if let Ok(mut pins) = PINS.lock() {
        pins.insert(
            (agent_id.to_string(), provider.to_string()),
            Pin {
                credential_id: credential_id.to_string(),
                expires_at_ms,
            },
        );
    }
}

/// Move the pinned credential to the front of the candidate list.
///
/// Reorder only: the pin never makes an ineligible credential eligible, and
/// an explicit billing target has already reduced the list to one row, so it
/// is untouched. A pinned credential reporting a full window is not
/// promoted -- the provider's own numbers say the next call there is the one
/// that buys the 429, and the block that answer writes is what the pin exists
/// to avoid paying for.
fn apply_pin(rows: &mut [broker::SubscriptionEntry], agent_id: &str, provider: &str) {
    let Some(pinned) = pinned_credential(agent_id, provider) else {
        return;
    };
    if usage::used_fraction(&pinned).is_some_and(|fraction| fraction >= 1.0) {
        return;
    }
    if let Some(position) = rows.iter().position(|entry| entry.id == pinned) {
        rows.swap(0, position);
    }
}

/// The candidate list `best` walks: every subscription model this agent can be
/// served from, freest plan first, with the alias's configured route promoted
/// to the head when the agent actually holds it.
///
/// The configured route is a preference, not the whole list. An operator
/// pointing `best` at `codex/gpt-5.3-codex-spark` is naming the model they want
/// first, not consenting to a fleet outage every time one provider's credential
/// chain breaks.
async fn best_subscription_models(
    agent_id: &str,
    preferred: Option<&str>,
) -> Result<Vec<String>, String> {
    let mut models = active_supported_models_for_agent(agent_id).await?;
    if let Some(position) =
        preferred.and_then(|preferred| models.iter().position(|model| model == preferred))
    {
        // Rotate rather than swap: everything behind the preferred route keeps
        // the plan order the ledger just computed for it.
        models[..=position].rotate_right(1);
    }
    Ok(models)
}

pub async fn active_supported_models_for_agent(agent_id: &str) -> Result<Vec<String>, String> {
    let mut models = registry_models_for_agent(agent_id)
        .await?
        .into_iter()
        .map(|model| model.route_id)
        .collect::<Vec<_>>();
    if models.is_empty() {
        return Err("no active stateless provider models for signed agent".into());
    }
    order_models_by_plan(agent_id, &mut models).await?;
    Ok(models)
}

pub async fn active_vision_capable_models_for_agent(agent_id: &str) -> Result<Vec<String>, String> {
    let mut models = registry_models_for_agent(agent_id)
        .await?
        .into_iter()
        .filter(|model| model.input_modalities.iter().any(|value| value == "image"))
        .map(|model| model.route_id)
        .collect::<Vec<_>>();
    if models.is_empty() {
        return Err("no active vision-capable stateless provider model for signed agent".into());
    }
    order_models_by_plan(agent_id, &mut models).await?;
    Ok(models)
}

fn score_field(row: &Value) -> Option<f64> {
    row.get("score").and_then(|score| score.as_f64())
}

fn model_field(row: &Value) -> Option<String> {
    row.get("model")
        .and_then(|model| model.as_str())
        .map(|model| model.trim().to_string())
        .filter(|model| !model.is_empty())
}

fn checked_at_field(row: &Value) -> i64 {
    let value = string_field(row, "checked_at");
    chrono::DateTime::parse_from_rfc3339(&value)
        .map(|dt| dt.timestamp())
        .unwrap_or(0)
}

async fn task_quality_models(agent_id: &str, task: &str) -> Result<Vec<String>, String> {
    let active_models = active_supported_models_for_agent(agent_id).await?;
    let rows = crate::journal::checks_for_task(agent_id, task);
    if rows.is_empty() {
        return Err(format!("no quality checks configured for task '{task}'"));
    }
    let mut latest_by_model: HashMap<String, (f64, i64)> = HashMap::new();
    for row in rows
        .iter()
        .filter(|row| string_field(row, "status") == "active")
    {
        let Some(model) = model_field(row) else {
            continue;
        };
        if !active_models.iter().any(|active| active == &model) {
            continue;
        }
        let Some(score) = score_field(row) else {
            continue;
        };
        let checked_at = checked_at_field(row);
        match latest_by_model.get(&model) {
            Some((_, existing_checked_at)) if *existing_checked_at >= checked_at => {}
            _ => {
                latest_by_model.insert(model, (score, checked_at));
            }
        }
    }
    let mut scored = latest_by_model
        .into_iter()
        .map(|(model, (score, checked_at))| (model, score, checked_at))
        .collect::<Vec<_>>();
    if scored.is_empty() {
        return Err(format!(
            "no active quality check result for task '{task}' and signed agent"
        ));
    }
    scored.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| b.2.cmp(&a.2))
    });
    let subscriptions = broker::list_subscriptions(agent_id).await;
    let mut ordered = Vec::new();
    let mut idx = 0;
    while idx < scored.len() {
        let score = scored[idx].1;
        let mut group = Vec::new();
        while idx < scored.len() && (scored[idx].1 - score).abs() < f64::EPSILON {
            group.push(scored[idx].0.clone());
            idx += 1;
        }
        // Quality outranks quota: inside one score the freest plan leads, and
        // only two models whose providers are equally spent are shuffled.
        group.sort_by(|left, right| {
            route_plan_key(&subscriptions, left)
                .partial_cmp(&route_plan_key(&subscriptions, right))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        shuffle_within_equal(&mut group, |model| {
            ordered_float_key(route_plan_key(&subscriptions, model))
        })?;
        ordered.extend(group);
    }
    Ok(ordered)
}

/// The sentence a selector that walked its whole candidate list opens with.
/// One string per selector, shared by the buffered and streaming walks, so the
/// two cannot drift into describing the same exhausted list differently.
const ANY_SUBSCRIPTION_CONTEXT: &str = "no working subscription model for signed agent";
const ANY_VISION_CONTEXT: &str = "no working vision-capable subscription model for signed agent";

/// What one ranked walk has spent and what refused it so far.
///
/// A ranked selector is a list of routes, and several of those routes belong to
/// the same provider. When a provider's whole credential pool empties, every
/// remaining route of that provider will be refused for the identical reason,
/// so re-dispatching them buys nothing and -- because the model budget is
/// small -- costs the caller every candidate that could still have served. That
/// is exactly how `best` answered `503` naming codex with `attempts: 0` while
/// kimi was serving in the same second.
///
/// So the walk remembers three things: which providers have already emptied,
/// how many provider round trips have actually been paid for, and what each
/// refusal said. A refusal that never reached a provider costs no budget --
/// there is nothing to bound -- and a provider named once is not named twice.
#[derive(Default)]
struct RankedWalk {
    attempts: u32,
    provider_calls: usize,
    refusals: Vec<String>,
    emptied: Vec<String>,
}

impl RankedWalk {
    /// Whether this candidate belongs to a provider that has already refused
    /// everything it holds for this agent.
    fn already_emptied(&self, provider: &str) -> bool {
        self.emptied.iter().any(|seen| seen == provider)
    }

    /// Whether the walk has paid for as many provider round trips as one
    /// selector is allowed to spend.
    fn budget_spent(&self) -> bool {
        self.provider_calls >= max_selector_models()
    }

    /// Record one refused candidate: its cost, its reason, and -- when the
    /// refusal was the provider's whole pool rather than this one route -- the
    /// fact that the rest of that provider's routes need not be asked.
    fn refused(&mut self, model: &str, provider: &str, response: &ModelResponse, emptied: bool) {
        self.attempts = self.attempts.saturating_add(response.attempts);
        if response.attempts > u32::default() {
            self.provider_calls = self.provider_calls.saturating_add(usize::from(true));
        }
        let reason = response.error.as_deref().unwrap_or("failed");
        warn!(
            event = "ranked_candidate_refused",
            model,
            provider,
            pool_emptied = emptied,
            attempts = response.attempts,
            reason,
            "ranked selector walked past a refused candidate"
        );
        if emptied {
            if self.already_emptied(provider) {
                return;
            }
            self.emptied.push(provider.to_owned());
            // The provider's name, not the route's: the pool refused every
            // model behind it, and naming one of them would read as if the
            // others were untried.
            self.refusals.push(format!("{provider} refused ({reason})"));
            return;
        }
        self.refusals.push(format!("{model} refused ({reason})"));
    }

    /// Note a candidate skipped without a provider call, so the log shows the
    /// walk passing it rather than never reaching it.
    fn skipped(&self, model: &str, provider: &str) {
        info!(
            event = "ranked_candidate_skipped",
            model,
            provider,
            "provider pool already emptied for this request; candidate skipped unasked"
        );
    }

    /// The refusal an exhausted candidate list answers with: every provider
    /// that was walked past, each with the reason it gave, and the number of
    /// provider attempts that were really made.
    fn into_failure(self, request: &ModelRequest, context: &str) -> ModelResponse {
        let message = if self.refusals.is_empty() {
            context.to_owned()
        } else {
            format!("{context}; {}", self.refusals.join(", "))
        };
        let mut failure = refuse(request, POINT_MODEL_SELECTION, message, None);
        failure.attempts = self.attempts;
        failure
    }
}

/// Walk a ranked candidate list until one of them serves.
///
/// A candidate that cannot be redeemed is walked past, not fatal: only an
/// exhausted list is a failed request, and the failure names every provider
/// the walk went past and what each of them said.
async fn dispatch_ranked_models(
    agent_id: &str,
    request: &ModelRequest,
    models: Vec<String>,
    failure_context: &str,
) -> ModelResponse {
    let mut walk = RankedWalk::default();
    for model in models {
        let Some(provider) = provider_for(&model) else {
            walk.refusals
                .push(format!("{model} refused (unknown provider/model route)"));
            continue;
        };
        if walk.already_emptied(provider) {
            walk.skipped(&model, provider);
            continue;
        }
        if walk.budget_spent() {
            break;
        }
        let mut candidate = request.clone();
        candidate.model = model.clone();
        let attempt = attempt_subscription(provider, agent_id, &candidate).await;
        match attempt.opened {
            Ok(mut served) => {
                served.attempts = walk.attempts.saturating_add(served.attempts);
                return served;
            }
            Err(response) => walk.refused(&model, provider, &response, attempt.pool_emptied),
        }
    }
    walk.into_failure(request, failure_context)
}

/// `model: "any"` selects among active stateless provider routes for the
/// signed agent and rotates across credentials on provider exhaustion.
pub async fn dispatch_any_subscription(
    headers: &HeaderMap,
    request: &ModelRequest,
    raw_body: &[u8],
) -> ModelResponse {
    let agent_id = match authenticate_agent(headers, raw_body).await {
        Ok(agent_id) => agent_id,
        Err(e) => return ModelResponse::failure(&request.model, e),
    };
    let models = match active_supported_models_for_agent(&agent_id).await {
        Ok(models) => models,
        Err(e) => return ModelResponse::failure(&request.model, e),
    };
    dispatch_ranked_models(&agent_id, request, models, ANY_SUBSCRIPTION_CONTEXT).await
}

/// `best` is a selector, not a route: it means the best subscription model this
/// signed caller can actually be served from right now.
///
/// The alias resolves to one configured provider route and that route leads the
/// list, but it has never been the only thing `best` may answer with. Dispatching
/// it alone is what turned one unredeemable codex credential into a `503` for a
/// fleet holding three live subscription providers.
pub async fn dispatch_best_subscription(
    headers: &HeaderMap,
    request: &ModelRequest,
    raw_body: &[u8],
    preferred: Option<&str>,
) -> ModelResponse {
    let agent_id = match authenticate_agent(headers, raw_body).await {
        Ok(agent_id) => agent_id,
        Err(e) => return ModelResponse::failure(&request.model, e),
    };
    dispatch_best_subscription_for_agent(&agent_id, request, preferred).await
}

pub async fn dispatch_best_subscription_for_agent(
    agent_id: &str,
    request: &ModelRequest,
    preferred: Option<&str>,
) -> ModelResponse {
    let models = match best_subscription_models(agent_id, preferred).await {
        Ok(models) => models,
        Err(e) => return ModelResponse::failure(&request.model, e),
    };
    dispatch_ranked_models(agent_id, request, models, ANY_SUBSCRIPTION_CONTEXT).await
}

/// `model: "any-vision-capable"` selects an active stateless provider route
/// whose catalog metadata advertises image input.
pub async fn dispatch_any_vision_capable_subscription(
    headers: &HeaderMap,
    request: &ModelRequest,
    raw_body: &[u8],
) -> ModelResponse {
    let agent_id = match authenticate_agent(headers, raw_body).await {
        Ok(agent_id) => agent_id,
        Err(e) => return ModelResponse::failure(&request.model, e),
    };
    let models = match active_vision_capable_models_for_agent(&agent_id).await {
        Ok(models) => models,
        Err(e) => return ModelResponse::failure(&request.model, e),
    };
    dispatch_ranked_models(&agent_id, request, models, ANY_VISION_CONTEXT).await
}

/// `model: "task:<name>"` means: use measured quality evidence for `<name>`.
/// The router does not infer tasks from prompt text. It only uses persisted
/// rows written by the task-quality collector.
pub async fn dispatch_task_subscription(
    headers: &HeaderMap,
    request: &ModelRequest,
    raw_body: &[u8],
    task: &str,
) -> ModelResponse {
    let agent_id = match authenticate_agent(headers, raw_body).await {
        Ok(id) => id,
        Err(e) => return ModelResponse::failure(&request.model, e),
    };
    let models = match task_quality_models(&agent_id, task).await {
        Ok(models) => models,
        Err(e) => return ModelResponse::failure(&request.model, e),
    };
    dispatch_ranked_models(
        &agent_id,
        request,
        models,
        &format!("no working quality-ranked model for task '{task}'"),
    )
    .await
}

pub async fn dispatch_subscription_for_agent(
    agent_id: &str,
    request: &ModelRequest,
) -> ModelResponse {
    let provider = match provider_for(&request.model) {
        Some(provider) => provider,
        None => {
            return ModelResponse::failure(&request.model, "unknown provider/model route".into())
        }
    };
    dispatch_subscription_attempt(provider, agent_id, request).await
}

pub async fn dispatch_subscription_stream_for_agent(
    agent_id: &str,
    request: &ModelRequest,
) -> Result<RoutedStream, ModelResponse> {
    let provider = match provider_for(&request.model) {
        Some(provider) => provider,
        None => {
            return Err(ModelResponse::failure(
                &request.model,
                "unknown provider/model route".into(),
            ))
        }
    };
    attempt_subscription_stream(provider, agent_id, request)
        .await
        .opened
}

/// Spend one provider call on exactly one subscription, for the on-demand usage
/// probe.
///
/// Three differences from the request path, each of them deliberate. It never
/// rotates: a probe asks a named account what its plan says, and answering with
/// the window of whichever sibling credential happened to work would attribute
/// one account's quota to another. It returns the provider's own sentence rather
/// than the rotation summary, because the sentence -- "OAuth access token has
/// been revoked" against "token is expired" -- is the entire reason the probe
/// exists. And it neither forces a refresh nor retires anything: a check an
/// operator asked for must not change which credentials the router will pick. An
/// access token that merely expired is still renewed, because redemption does
/// that for every caller before the call is made.
///
/// What it shares is the part that matters: the same provider dispatch and the
/// same ledger path real traffic writes to, so a probe's plan windows are
/// recorded exactly as a request's are -- attributed to the probe, because a
/// window somebody asked for and a window a caller's work revealed are not the
/// same statement about the account.
pub async fn probe_subscription_usage(
    subscription_id: &str,
    provider: &str,
    request: &ModelRequest,
) -> ModelResponse {
    let token = match broker::subscription_credential(subscription_id, provider).await {
        Ok(token) => token,
        Err(refused) => {
            return refuse(
                request,
                POINT_CREDENTIAL_SELECTION,
                failure_detail(&refused),
                Some(refused),
            );
        }
    };
    let token = match token.expose_utf8() {
        Ok(token) => token,
        Err(error) => {
            return refuse(
                request,
                POINT_CREDENTIAL_SELECTION,
                format!("credential is not valid UTF-8: {error}"),
                None,
            );
        }
    };
    let item = broker::subscription_resource(provider, subscription_id);
    let mut result = provider_registry::dispatch(request, &item, token).await;
    result.attempts = u32::from(true);
    usage::record_call_from(
        subscription_id,
        provider,
        &result,
        usage::UsageSource::Probe,
    );
    result
}

/// Execute a caller-independent canonical route with Brama's dedicated direct
/// provider capability. Subscription provider credentials are never eligible.
pub async fn dispatch_direct(request: &ModelRequest) -> ModelResponse {
    let provider = match provider_for(&request.model) {
        Some(provider) => provider,
        None => {
            return ModelResponse::failure(&request.model, "unknown provider/model route".into())
        }
    };
    if provider_requires_caller_identity(&request.model) {
        return ModelResponse::failure(
            &request.model,
            "auth: caller identity is required for subscription providers".into(),
        );
    }
    let credential = match broker::provider_credential(provider).await {
        Some(credential) => credential,
        None => {
            return ModelResponse::failure(
                &request.model,
                format!("direct '{provider}' credential is unavailable"),
            )
        }
    };
    let credential = match credential.expose_utf8() {
        Ok(credential) => credential,
        Err(_) => {
            return ModelResponse::failure(
                &request.model,
                format!("direct '{provider}' credential is not valid UTF-8"),
            )
        }
    };
    provider_registry::dispatch(request, &broker::provider_resource(provider), credential).await
}

/// Execute an ordered caller-independent route chain. A failed primary is
/// followed by each configured fallback; attempt accounting spans the chain.
pub async fn dispatch_direct_with_fallback(
    request: &ModelRequest,
    fallbacks: &[String],
) -> ModelResponse {
    let mut response = dispatch_direct(request).await;
    let mut attempts = response.attempts;
    for fallback in fallbacks {
        if response.success {
            break;
        }
        let mut next = request.clone();
        next.model = fallback.clone();
        response = dispatch_direct(&next).await;
        attempts = attempts.saturating_add(response.attempts);
    }
    response.attempts = attempts;
    response
}

pub async fn dispatch_direct_openai_typed(
    route_id: &str,
    path: &str,
    payload: serde_json::Map<String, Value>,
) -> Result<Value, String> {
    let provider =
        provider_for(route_id).ok_or_else(|| "unknown provider/model route".to_string())?;
    if provider_requires_caller_identity(route_id) {
        return Err("auth: caller identity is required for subscription providers".to_string());
    }
    let credential = broker::provider_credential(provider)
        .await
        .ok_or_else(|| format!("direct '{provider}' credential is unavailable"))?;
    let credential = credential
        .expose_utf8()
        .map_err(|_| format!("direct '{provider}' credential is not valid UTF-8"))?;
    provider_registry::dispatch_openai_typed(
        route_id,
        path,
        payload,
        &broker::provider_resource(provider),
        credential,
    )
    .await
}

/// Authenticate the Jeden caller, redeem the selected provider credential at
/// the final-use boundary, and execute one stateless provider API request.
pub async fn dispatch_subscription(
    headers: &HeaderMap,
    request: &ModelRequest,
    raw_body: &[u8],
) -> ModelResponse {
    let provider = match provider_for(&request.model) {
        Some(provider) => provider,
        None => {
            return ModelResponse::failure(&request.model, "unknown provider/model route".into())
        }
    };
    let agent_id = match authenticate_agent(headers, raw_body).await {
        Ok(agent_id) => agent_id,
        Err(error) => return ModelResponse::failure(&request.model, error),
    };
    dispatch_subscription_attempt(provider, &agent_id, request).await
}

/// One provider's answer to one candidate route, and whether the refusal
/// emptied that provider's whole pool for this agent rather than failing this
/// one route.
///
/// A ranked walk needs the distinction and cannot recover it from the sentence:
/// an emptied pool refuses every remaining route of the provider identically,
/// while a route the provider rejected on its own merits says nothing about the
/// provider's other models.
struct RouteAttempt<T> {
    opened: Result<T, ModelResponse>,
    pool_emptied: bool,
}

impl<T> RouteAttempt<T> {
    fn served(opened: T) -> Self {
        Self {
            opened: Ok(opened),
            pool_emptied: false,
        }
    }

    /// A refusal that belongs to this route alone.
    fn refused(failure: ModelResponse) -> Self {
        Self {
            opened: Err(failure),
            pool_emptied: false,
        }
    }

    /// A refusal that is this provider's pool having nothing left for the agent.
    fn pool_empty(failure: ModelResponse) -> Self {
        Self {
            opened: Err(failure),
            pool_emptied: true,
        }
    }
}

async fn dispatch_subscription_attempt(
    provider: &str,
    agent_id: &str,
    request: &ModelRequest,
) -> ModelResponse {
    match attempt_subscription(provider, agent_id, request)
        .await
        .opened
    {
        Ok(served) => served,
        Err(failure) => failure,
    }
}

async fn attempt_subscription(
    provider: &str,
    agent_id: &str,
    request: &ModelRequest,
) -> RouteAttempt<ModelResponse> {
    let mut rows = match eligible_subscription_entries(
        broker::list_subscriptions(agent_id).await,
        provider,
        request.billing_target.as_ref(),
    ) {
        Ok(rows) => rows,
        Err(error) => {
            return RouteAttempt::pool_empty(ModelResponse::failure(&request.model, error))
        }
    };
    if rows.is_empty() {
        return RouteAttempt::pool_empty(refuse(
            request,
            POINT_CREDENTIAL_SELECTION,
            request.billing_target.as_ref().map_or_else(
                || no_active_credential_summary(provider),
                |target| {
                    format!(
                        "selected credential '{}' is not active for provider '{provider}' and agent",
                        target.subscription_id
                    )
                },
            ),
            None,
        ));
    }

    // Freest plan first, the ledger's own numbers rather than list order; the
    // agent's pinned credential then leads unless its window says it is full.
    // Both are reorderings of the same bounded list -- the attempt cap below
    // is untouched, and an explicit billing target has one row to reorder.
    rows.sort_by(|left, right| {
        usage::used_fraction(&left.id)
            .unwrap_or(0.0)
            .partial_cmp(&usage::used_fraction(&right.id).unwrap_or(0.0))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    apply_pin(&mut rows, agent_id, provider);

    let mut provider_attempts = u32::default();
    // The newest credential-boundary refusal is the operation that finally
    // stopped this request, whether redemption, refresh, or credential decode.
    let mut credential_refusal: Option<Failure> = None;
    // Set when a provider refused a credential outright, as opposed to being out
    // of quota: the two empty the pool for different reasons and the caller has
    // to be told which.
    let mut saw_auth_rejection = false;
    // Set when the vault produced no credential at all -- no capability, no
    // grant, nothing to present. No provider was ever asked, so this is neither
    // capacity nor a provider refusal, and the caller must not be told to wait
    // for it.
    let mut saw_unredeemable_credential = false;
    // Set when a credential was skipped because its recorded block is an
    // authorization block rather than a rate limit. The ledger knows the
    // difference -- `record_reauthorization_needed` writes the state beside the
    // block precisely so it is not lost -- and without reading it here the
    // whole half hour reports as capacity.
    let mut saw_reauthorization_block = false;
    let mut saw_rate_limit_block = false;
    let mut rate_limit_failure = None;
    for (index, entry) in rows.iter().take(max_credential_attempts()).enumerate() {
        let credential_id = &entry.id;
        // A credential inside a recorded block is skipped without a provider
        // call. The previous behaviour re-derived exhaustion from an error
        // string on every request, which meant paying for the 429 to learn what
        // the last 429 already said.
        if usage::is_blocked(credential_id) {
            let reauthorization = usage::needs_reauthorization(credential_id);
            saw_reauthorization_block = saw_reauthorization_block || reauthorization;
            saw_rate_limit_block |= !reauthorization;
            warn!(
                event = "credential_blocked",
                provider,
                credential_index = index,
                reauthorization,
                "bounded credential is inside a recorded block"
            );
            continue;
        }
        let token = match broker::subscription_credential(credential_id, provider).await {
            Ok(token) => token,
            Err(refused) => {
                saw_unredeemable_credential = true;
                warn!(
                    event = "credential_unavailable",
                    provider,
                    credential_index = index,
                    envelope = %refused.to_json(),
                    "{}",
                    refused.render()
                );
                remember_failure(&mut credential_refusal, refused);
                continue;
            }
        };
        let token = match token.expose_utf8() {
            Ok(token) => token,
            Err(error) => {
                saw_unredeemable_credential = true;
                let refused = failure::envelope(
                    POINT_CREDENTIAL_SELECTION,
                    failure::code_for("credential_unauthorized"),
                    IMPACT_MODEL_REQUEST,
                    format!("subscription credential is not valid UTF-8: {error}"),
                )
                .with_context("subscription", credential_id)
                .with_context("provider", provider);
                warn!(
                    event = "credential_invalid_encoding",
                    provider,
                    credential_index = index,
                    envelope = %refused.to_json(),
                    "{}",
                    refused.render()
                );
                remember_failure(&mut credential_refusal, refused);
                continue;
            }
        };
        provider_attempts = provider_attempts.saturating_add(u32::from(true));
        let item = broker::subscription_resource(provider, credential_id);
        let mut result = provider_registry::dispatch(request, &item, token).await;
        result.attempts = provider_attempts;
        usage::record_call(credential_id, provider, &result);

        // Whether the provider refused a token it had just handed us. The
        // string matcher below cannot see that: the message is the same
        // "invalid authentication" either way, and only the sequence says the
        // credential is dead rather than stale.
        let mut rejected_with_fresh_token = false;
        if !result.success && result.error.as_deref().is_some_and(is_auth_failure) {
            match broker::refresh_subscription_credential(credential_id, provider).await {
                Ok(fresh) => match fresh.expose_utf8() {
                    Ok(fresh_token) => {
                        provider_attempts = provider_attempts.saturating_add(u32::from(true));
                        result = provider_registry::dispatch(request, &item, fresh_token).await;
                        result.attempts = provider_attempts;
                        usage::record_call(credential_id, provider, &result);
                        if result.success {
                            info!(
                                event = "credential_refreshed",
                                provider,
                                credential_index = index,
                                "provider request succeeded after forced OAuth refresh"
                            );
                            pin_credential(agent_id, provider, credential_id);
                            return RouteAttempt::served(result);
                        }
                        rejected_with_fresh_token =
                            result.error.as_deref().is_some_and(is_auth_failure);
                    }
                    Err(error) => {
                        let refused = failure::envelope(
                            POINT_CREDENTIAL_SELECTION,
                            failure::code_for("credential_unauthorized"),
                            IMPACT_MODEL_REQUEST,
                            format!("refreshed credential is not valid UTF-8: {error}"),
                        )
                        .with_context("subscription", credential_id)
                        .with_context("provider", provider);
                        remember_failure(&mut credential_refusal, refused);
                    }
                },
                Err(refused) => remember_failure(&mut credential_refusal, refused),
            }
        }
        if result.success {
            if index > 0 {
                info!(
                    event = "credential_rotated",
                    provider,
                    credential_index = index,
                    "provider request succeeded after bounded rotation"
                );
            }
            pin_credential(agent_id, provider, credential_id);
            return RouteAttempt::served(result);
        }
        let error = result.error.clone().unwrap_or_default();
        if is_permanent_auth_failure(&error) {
            warn!(
                event = "credential_retired",
                provider,
                credential_index = index,
                "provider permanently rejected bounded credential"
            );
            mark_credential_revoked(
                credential_id,
                provider,
                "the provider permanently rejected this credential",
            )
            .await;
        }
        if rejected_with_fresh_token {
            // Retried forever otherwise: the refusal reads as an ordinary auth
            // failure, so the next request refreshes and is refused again. One
            // host wrote 275,468 of these lines against a single subscription
            // while every request paid for two provider round trips first.
            warn!(
                event = "credential_retired",
                provider,
                credential_index = index,
                "provider refused a token it had just issued; the subscription needs \
                 re-authorization and is retired until it is granted"
            );
            mark_credential_revoked(
                credential_id,
                provider,
                "the provider refused a token it had just issued",
            )
            .await;
        }
        if is_auth_failure(&error) {
            saw_auth_rejection = true;
            warn!(
                event = "credential_auth_rejected",
                provider,
                credential_index = index,
                "provider rejected bounded credential after OAuth refresh"
            );
            continue;
        }
        let exhausted = error.contains("hit your limit")
            || error.contains("usage limit")
            || error.contains("rate_limit")
            || error.contains("429");
        if !exhausted {
            // The provider answered and refused this request in particular --
            // a malformed body, a model it does not serve. Its other
            // credentials would answer identically, and its other models might
            // not, so this refusal condemns the route and not the pool.
            return RouteAttempt::refused(result);
        }
        usage::record_block(credential_id, provider, &error, &result);
        warn!(
            event = "credential_exhausted",
            provider,
            credential_index = index,
            "provider rejected bounded credential with a rotatable failure"
        );
        rate_limit_failure = Some(result);
    }
    // A usable account's quota reset can serve the request even when another
    // account needs authorization. Preserve its actual provider refusal.
    if let Some(mut failure) = rate_limit_failure {
        failure.attempts = provider_attempts;
        return RouteAttempt::pool_empty(failure);
    }
    if saw_rate_limit_block {
        let mut failure = refuse(
            request,
            POINT_BOUNDED_ROTATION,
            bounded_unavailable_summary(provider),
            None,
        );
        failure.attempts = provider_attempts;
        return RouteAttempt::pool_empty(failure);
    }
    let cause = PoolEmptyCause {
        auth_rejection: saw_auth_rejection,
        reauthorization_block: saw_reauthorization_block,
        unredeemable_credential: saw_unredeemable_credential,
    };
    let message = credential_refusal
        .as_ref()
        .map(|refused| {
            format!(
                "'{provider}' subscription credential failed: {}",
                failure_detail(refused)
            )
        })
        .unwrap_or_else(|| pool_empty_summary(provider, cause));
    let mut failure = refuse_as(
        request,
        POINT_BOUNDED_ROTATION,
        rotation_failure_kind(cause),
        message,
        credential_refusal,
    );
    failure.attempts = provider_attempts;
    RouteAttempt::pool_empty(failure)
}

fn string_field(row: &Value, key: &str) -> String {
    match row.get(key) {
        Some(Value::String(value)) => value.trim().to_string(),
        Some(Value::Number(value)) => value.to_string(),
        Some(Value::Bool(value)) => value.to_string(),
        _ => String::new(),
    }
}

use crate::providers::stream::{ProviderStream, StreamDelta, StreamItem};
use tokio::sync::mpsc;

/// One committed, routed generation stream.
///
/// `model` and `attempts` are the facts the HTTP layer needs to finish its own
/// accounting; `events` is already past the rotation boundary -- every item on
/// it belongs to a provider answer that committed, and no item on it will ever
/// be followed by a silent retry on another credential.
pub struct RoutedStream {
    pub model: String,
    pub attempts: u32,
    pub events: mpsc::Receiver<StreamItem>,
}

/// Forward one committed provider stream to the caller's channel, recording
/// the subscription's spend when the stream ends, however it ends.
///
/// The buffered path records from a whole [`ModelResponse`]; a stream has no
/// whole response until it is over, so this task accumulates one. Three
/// endings all write exactly one record: `Done` is the measured generation,
/// `Failed` is a partial generation with the provider's own sentence, and a
/// closed caller channel is a cut the caller asked for -- still spend the
/// subscription paid for, so it is recorded as a failure with what was
/// measured rather than dropped from the ledger entirely.
fn spawn_stream_recorder(
    subscription_id: &str,
    provider: &str,
    model: &str,
    attempts: u32,
    started: Instant,
    stream: ProviderStream,
) -> mpsc::Receiver<StreamItem> {
    let (forward_tx, forward_rx) = mpsc::channel::<StreamItem>(64);
    let subscription_id = subscription_id.to_string();
    let provider = provider.to_string();
    let model = model.to_string();
    let limits = stream.limits;
    let mut events = stream.events;
    tokio::spawn(async move {
        let mut content = String::new();
        let mut input_tokens = u32::default();
        let mut output_tokens = u32::default();
        let mut error: Option<String> = None;
        let mut finished = false;
        loop {
            let item = match events.recv().await {
                Some(item) => item,
                None => break,
            };
            match &item {
                StreamItem::Delta(StreamDelta::Text(text)) => content.push_str(text),
                StreamItem::Delta(StreamDelta::Usage {
                    input_tokens: input,
                    output_tokens: output,
                }) => {
                    input_tokens = *input;
                    output_tokens = *output;
                }
                StreamItem::Failed(message) => error = Some(message.clone()),
                _ => {}
            }
            let terminal = matches!(item, StreamItem::Failed(_) | StreamItem::Done);
            finished = finished || matches!(item, StreamItem::Done);
            if forward_tx.send(item).await.is_err() {
                error.get_or_insert_with(|| "caller disconnected mid-stream".to_string());
                break;
            }
            if terminal {
                break;
            }
        }
        let success = finished && error.is_none();
        let mut response = ModelResponse::failure(
            &model,
            error.unwrap_or_else(|| "provider stream ended without a verdict".to_string()),
        );
        response.success = success;
        if success {
            response.error = None;
        }
        response.content = content;
        response.input_tokens = input_tokens;
        response.output_tokens = output_tokens;
        response.latency_ms = started.elapsed().as_secs_f64() * 1_000.0;
        response.attempts = attempts;
        response.limits = limits;
        usage::record_call(&subscription_id, &provider, &response);
    });
    forward_rx
}

/// Authenticate the Jeden caller and open one streaming subscription
/// generation, rotating across bounded credentials exactly as the buffered
/// path does -- except every rotation happens before the first caller byte,
/// because after it none is possible.
pub async fn dispatch_subscription_stream(
    headers: &HeaderMap,
    request: &ModelRequest,
    raw_body: &[u8],
) -> Result<RoutedStream, ModelResponse> {
    let provider = match provider_for(&request.model) {
        Some(provider) => provider,
        None => {
            return Err(ModelResponse::failure(
                &request.model,
                "unknown provider/model route".into(),
            ))
        }
    };
    let agent_id = match authenticate_agent(headers, raw_body).await {
        Ok(agent_id) => agent_id,
        Err(error) => return Err(ModelResponse::failure(&request.model, error)),
    };
    attempt_subscription_stream(provider, &agent_id, request)
        .await
        .opened
}

async fn attempt_subscription_stream(
    provider: &str,
    agent_id: &str,
    request: &ModelRequest,
) -> RouteAttempt<RoutedStream> {
    let mut rows = match eligible_subscription_entries(
        broker::list_subscriptions(agent_id).await,
        provider,
        request.billing_target.as_ref(),
    ) {
        Ok(rows) => rows,
        Err(error) => {
            return RouteAttempt::pool_empty(ModelResponse::failure(&request.model, error))
        }
    };
    if rows.is_empty() {
        return RouteAttempt::pool_empty(refuse(
            request,
            POINT_CREDENTIAL_SELECTION,
            request.billing_target.as_ref().map_or_else(
                || no_active_credential_summary(provider),
                |target| {
                    format!(
                        "selected credential '{}' is not active for provider '{provider}' and agent",
                        target.subscription_id
                    )
                },
            ),
            None,
        ));
    }
    rows.sort_by(|left, right| {
        usage::used_fraction(&left.id)
            .unwrap_or(0.0)
            .partial_cmp(&usage::used_fraction(&right.id).unwrap_or(0.0))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    apply_pin(&mut rows, agent_id, provider);

    let mut provider_attempts = u32::default();
    let mut credential_refusal: Option<Failure> = None;
    let mut saw_auth_rejection = false;
    let mut saw_unredeemable_credential = false;
    // The streaming path empties its pool the same way the buffered one does,
    // so it reads the ledger's authorization state the same way. Fixing one and
    // not the other would make the same broken credential answer `503` to a
    // buffered caller and `429` to a streaming one.
    let mut saw_reauthorization_block = false;
    let mut saw_rate_limit_block = false;
    let mut rate_limit_failure = None;
    for (index, entry) in rows.iter().take(max_credential_attempts()).enumerate() {
        let credential_id = &entry.id;
        if usage::is_blocked(credential_id) {
            let reauthorization = usage::needs_reauthorization(credential_id);
            saw_reauthorization_block = saw_reauthorization_block || reauthorization;
            saw_rate_limit_block |= !reauthorization;
            warn!(
                event = "credential_blocked",
                provider,
                credential_index = index,
                reauthorization,
                "bounded credential is inside a recorded block"
            );
            continue;
        }
        let token = match broker::subscription_credential(credential_id, provider).await {
            Ok(token) => token,
            Err(refused) => {
                saw_unredeemable_credential = true;
                warn!(
                    event = "credential_unavailable",
                    provider,
                    credential_index = index,
                    envelope = %refused.to_json(),
                    "{}",
                    refused.render()
                );
                remember_failure(&mut credential_refusal, refused);
                continue;
            }
        };
        let token = match token.expose_utf8() {
            Ok(token) => token,
            Err(error) => {
                saw_unredeemable_credential = true;
                let refused = failure::envelope(
                    POINT_CREDENTIAL_SELECTION,
                    failure::code_for("credential_unauthorized"),
                    IMPACT_MODEL_REQUEST,
                    format!("subscription credential is not valid UTF-8: {error}"),
                )
                .with_context("subscription", credential_id)
                .with_context("provider", provider);
                warn!(
                    event = "credential_invalid_encoding",
                    provider,
                    credential_index = index,
                    envelope = %refused.to_json(),
                    "{}",
                    refused.render()
                );
                remember_failure(&mut credential_refusal, refused);
                continue;
            }
        };
        provider_attempts = provider_attempts.saturating_add(u32::from(true));
        let item = broker::subscription_resource(provider, credential_id);
        let started = Instant::now();
        let mut result = provider_registry::dispatch_stream(request, &item, token).await;
        // An auth refusal before the first byte gets the same one forced
        // refresh the buffered path allows, and the retry is just as safe:
        // the caller has seen nothing either way.
        let mut rejected_with_fresh_token = false;
        if result
            .as_ref()
            .err()
            .and_then(|failure| failure.error.as_deref())
            .is_some_and(is_auth_failure)
        {
            match broker::refresh_subscription_credential(credential_id, provider).await {
                Ok(fresh) => match fresh.expose_utf8() {
                    Ok(fresh_token) => {
                        provider_attempts = provider_attempts.saturating_add(u32::from(true));
                        result =
                            provider_registry::dispatch_stream(request, &item, fresh_token).await;
                        rejected_with_fresh_token = result
                            .as_ref()
                            .err()
                            .and_then(|failure| failure.error.as_deref())
                            .is_some_and(is_auth_failure);
                    }
                    Err(error) => {
                        let refused = failure::envelope(
                            POINT_CREDENTIAL_SELECTION,
                            failure::code_for("credential_unauthorized"),
                            IMPACT_MODEL_REQUEST,
                            format!("refreshed credential is not valid UTF-8: {error}"),
                        )
                        .with_context("subscription", credential_id)
                        .with_context("provider", provider);
                        remember_failure(&mut credential_refusal, refused);
                    }
                },
                Err(refused) => remember_failure(&mut credential_refusal, refused),
            }
        }
        let failure = match result {
            Ok(stream) => {
                if index > 0 {
                    info!(
                        event = "credential_rotated",
                        provider,
                        credential_index = index,
                        "provider stream committed after bounded rotation"
                    );
                }
                pin_credential(agent_id, provider, credential_id);
                return RouteAttempt::served(RoutedStream {
                    model: request.model.clone(),
                    attempts: provider_attempts,
                    events: spawn_stream_recorder(
                        credential_id,
                        provider,
                        &request.model,
                        provider_attempts,
                        started,
                        stream,
                    ),
                });
            }
            Err(failure) => failure,
        };
        usage::record_call(credential_id, provider, &failure);
        let error = failure.error.clone().unwrap_or_default();
        if is_permanent_auth_failure(&error) {
            warn!(
                event = "credential_retired",
                provider,
                credential_index = index,
                "provider permanently rejected bounded credential"
            );
            mark_credential_revoked(
                credential_id,
                provider,
                "the provider permanently rejected this credential",
            )
            .await;
        }
        if rejected_with_fresh_token {
            // Same loop guard as the buffered path: a refusal of a token the
            // provider had just issued reads as an ordinary auth failure and
            // would otherwise be refreshed and refused on every request.
            warn!(
                event = "credential_retired",
                provider,
                credential_index = index,
                "provider refused a token it had just issued; the subscription needs \
                 re-authorization and is retired until it is granted"
            );
            mark_credential_revoked(
                credential_id,
                provider,
                "the provider refused a token it had just issued",
            )
            .await;
        }
        if is_auth_failure(&error) {
            saw_auth_rejection = true;
            warn!(
                event = "credential_auth_rejected",
                provider,
                credential_index = index,
                "provider rejected bounded credential after OAuth refresh"
            );
            continue;
        }
        let exhausted = error.contains("hit your limit")
            || error.contains("usage limit")
            || error.contains("rate_limit")
            || error.contains("429");
        if !exhausted {
            // As in the buffered path: the provider answered and refused this
            // one route, which says nothing about its other models.
            return RouteAttempt::refused(failure);
        }
        usage::record_block(credential_id, provider, &error, &failure);
        warn!(
            event = "credential_exhausted",
            provider,
            credential_index = index,
            "provider rejected bounded credential with a rotatable failure"
        );
        rate_limit_failure = Some(failure);
    }
    // Keep the buffered path's precedence: a quota reset can restore service
    // without repairing every other account in the bounded pool.
    if let Some(mut failure) = rate_limit_failure {
        failure.attempts = provider_attempts;
        return RouteAttempt::pool_empty(failure);
    }
    if saw_rate_limit_block {
        let mut failure = refuse(
            request,
            POINT_BOUNDED_ROTATION,
            bounded_unavailable_summary(provider),
            None,
        );
        failure.attempts = provider_attempts;
        return RouteAttempt::pool_empty(failure);
    }
    let cause = PoolEmptyCause {
        auth_rejection: saw_auth_rejection,
        reauthorization_block: saw_reauthorization_block,
        unredeemable_credential: saw_unredeemable_credential,
    };
    let message = credential_refusal
        .as_ref()
        .map(|refused| {
            format!(
                "'{provider}' subscription credential failed: {}",
                failure_detail(refused)
            )
        })
        .unwrap_or_else(|| pool_empty_summary(provider, cause));
    let mut failure = refuse_as(
        request,
        POINT_BOUNDED_ROTATION,
        rotation_failure_kind(cause),
        message,
        credential_refusal,
    );
    failure.attempts = provider_attempts;
    RouteAttempt::pool_empty(failure)
}

/// The streaming counterpart of [`dispatch_ranked_models`]: the first model
/// whose stream commits wins, and a model that fails before its first byte
/// costs the caller nothing but an attempt count.
async fn dispatch_ranked_models_stream(
    agent_id: &str,
    request: &ModelRequest,
    models: Vec<String>,
    failure_context: &str,
) -> Result<RoutedStream, ModelResponse> {
    let mut walk = RankedWalk::default();
    for model in models {
        let Some(provider) = provider_for(&model) else {
            walk.refusals
                .push(format!("{model} refused (unknown provider/model route)"));
            continue;
        };
        if walk.already_emptied(provider) {
            walk.skipped(&model, provider);
            continue;
        }
        if walk.budget_spent() {
            break;
        }
        let mut candidate = request.clone();
        candidate.model = model.clone();
        let attempt = attempt_subscription_stream(provider, agent_id, &candidate).await;
        match attempt.opened {
            Ok(mut routed) => {
                routed.attempts = routed.attempts.saturating_add(walk.attempts);
                return Ok(routed);
            }
            Err(response) => walk.refused(&model, provider, &response, attempt.pool_emptied),
        }
    }
    Err(walk.into_failure(request, failure_context))
}

pub async fn dispatch_any_subscription_stream(
    headers: &HeaderMap,
    request: &ModelRequest,
    raw_body: &[u8],
) -> Result<RoutedStream, ModelResponse> {
    let agent_id = match authenticate_agent(headers, raw_body).await {
        Ok(agent_id) => agent_id,
        Err(e) => return Err(ModelResponse::failure(&request.model, e)),
    };
    let models = match active_supported_models_for_agent(&agent_id).await {
        Ok(models) => models,
        Err(e) => return Err(ModelResponse::failure(&request.model, e)),
    };
    dispatch_ranked_models_stream(&agent_id, request, models, ANY_SUBSCRIPTION_CONTEXT).await
}

/// The streaming counterpart of [`dispatch_best_subscription`]: the configured
/// route leads, and a provider that cannot open a stream is walked past exactly
/// as the buffered path walks past one that cannot answer.
pub async fn dispatch_best_subscription_stream(
    headers: &HeaderMap,
    request: &ModelRequest,
    raw_body: &[u8],
    preferred: Option<&str>,
) -> Result<RoutedStream, ModelResponse> {
    let agent_id = match authenticate_agent(headers, raw_body).await {
        Ok(agent_id) => agent_id,
        Err(e) => return Err(ModelResponse::failure(&request.model, e)),
    };
    dispatch_best_subscription_stream_for_agent(&agent_id, request, preferred).await
}

pub async fn dispatch_best_subscription_stream_for_agent(
    agent_id: &str,
    request: &ModelRequest,
    preferred: Option<&str>,
) -> Result<RoutedStream, ModelResponse> {
    let models = match best_subscription_models(agent_id, preferred).await {
        Ok(models) => models,
        Err(e) => return Err(ModelResponse::failure(&request.model, e)),
    };
    dispatch_ranked_models_stream(agent_id, request, models, ANY_SUBSCRIPTION_CONTEXT).await
}

pub async fn dispatch_any_vision_capable_subscription_stream(
    headers: &HeaderMap,
    request: &ModelRequest,
    raw_body: &[u8],
) -> Result<RoutedStream, ModelResponse> {
    let agent_id = match authenticate_agent(headers, raw_body).await {
        Ok(agent_id) => agent_id,
        Err(e) => return Err(ModelResponse::failure(&request.model, e)),
    };
    let models = match active_vision_capable_models_for_agent(&agent_id).await {
        Ok(models) => models,
        Err(e) => return Err(ModelResponse::failure(&request.model, e)),
    };
    dispatch_ranked_models_stream(&agent_id, request, models, ANY_VISION_CONTEXT).await
}

pub async fn dispatch_task_subscription_stream(
    headers: &HeaderMap,
    request: &ModelRequest,
    raw_body: &[u8],
    task: &str,
) -> Result<RoutedStream, ModelResponse> {
    let agent_id = match authenticate_agent(headers, raw_body).await {
        Ok(id) => id,
        Err(e) => return Err(ModelResponse::failure(&request.model, e)),
    };
    let models = match task_quality_models(&agent_id, task).await {
        Ok(models) => models,
        Err(e) => return Err(ModelResponse::failure(&request.model, e)),
    };
    dispatch_ranked_models_stream(
        &agent_id,
        request,
        models,
        &format!("no working quality-ranked model for task '{task}'"),
    )
    .await
}

/// Open one streaming generation on a direct route: one provider attempt, no
/// rotation, no subscription credential ever eligible.
pub async fn dispatch_direct_stream(request: &ModelRequest) -> Result<RoutedStream, ModelResponse> {
    let provider = match provider_for(&request.model) {
        Some(provider) => provider,
        None => {
            return Err(ModelResponse::failure(
                &request.model,
                "unknown provider/model route".into(),
            ))
        }
    };
    if provider_requires_caller_identity(&request.model) {
        return Err(ModelResponse::failure(
            &request.model,
            "auth: caller identity is required for subscription providers".into(),
        ));
    }
    let credential = match broker::provider_credential(provider).await {
        Some(credential) => credential,
        None => {
            return Err(ModelResponse::failure(
                &request.model,
                format!("direct '{provider}' credential is unavailable"),
            ))
        }
    };
    let credential = match credential.expose_utf8() {
        Ok(credential) => credential,
        Err(_) => {
            return Err(ModelResponse::failure(
                &request.model,
                format!("direct '{provider}' credential is not valid UTF-8"),
            ))
        }
    };
    let stream = provider_registry::dispatch_stream(
        request,
        &broker::provider_resource(provider),
        credential,
    )
    .await?;
    Ok(RoutedStream {
        model: request.model.clone(),
        attempts: u32::from(true),
        events: stream.events,
    })
}

/// The streaming counterpart of [`dispatch_direct_with_fallback`]: each
/// fallback is entered only while no byte has been produced.
pub async fn dispatch_direct_with_fallback_stream(
    request: &ModelRequest,
    fallbacks: &[String],
) -> Result<RoutedStream, ModelResponse> {
    let mut result = dispatch_direct_stream(request).await;
    let mut attempts = match &result {
        Ok(routed) => routed.attempts,
        Err(failure) => failure.attempts,
    };
    for fallback in fallbacks {
        if result.is_ok() {
            break;
        }
        let mut next = request.clone();
        next.model = fallback.clone();
        result = dispatch_direct_stream(&next).await;
        attempts = attempts.saturating_add(match &result {
            Ok(routed) => routed.attempts,
            Err(failure) => failure.attempts,
        });
    }
    match result {
        Ok(mut routed) => {
            routed.attempts = attempts;
            Ok(routed)
        }
        Err(mut failure) => {
            failure.attempts = attempts;
            Err(failure)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::redeemed_credential_verdict;

    /// The vault coordinate the 2026-09-02 disagreement was about.
    const SECONDARY: &str = "provider:codex:brama-sub-wisent-app-codex-secondary";

    /// The defect: readiness called this row redeemable while the same
    /// gateway's own model call could derive no key from it.
    #[test]
    fn account_metadata_is_not_a_redeemable_credential() {
        let refusal = redeemed_credential_verdict(SECONDARY, r#"{"type":"oauth_account"}"#)
            .expect_err("readiness accepted a document no request could present");
        assert!(
            refusal.contains(SECONDARY),
            "the refusal must name the coordinate the repair is at: {refusal}"
        );
        assert!(
            refusal.contains("carries no credential"),
            "the refusal must say what is wrong with the document: {refusal}"
        );
        assert!(
            refusal.contains("type"),
            "the refusal must name the fields the document does have: {refusal}"
        );
    }

    /// An OAuth grant with an access token is what a request can present, and
    /// the verdict must not leak it.
    #[test]
    fn an_oauth_grant_with_an_access_token_redeems() {
        let document = r#"{"tokens":{"access_token":"not-a-real-token"},"type":"oauth"}"#;
        assert!(redeemed_credential_verdict(SECONDARY, document).is_ok());
    }

    /// A bare key, the other shape the vault holds, still redeems.
    #[test]
    fn a_bare_secret_redeems() {
        assert!(redeemed_credential_verdict(SECONDARY, "sk-not-a-real-key").is_ok());
    }

    /// An empty row is a refusal, not a pass.
    #[test]
    fn an_empty_row_is_refused() {
        assert!(redeemed_credential_verdict(SECONDARY, "   ").is_err());
    }
}
