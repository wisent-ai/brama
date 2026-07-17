//! Route `"*-subscription"` chat completions requests to the matching CLI.

use aes_gcm::aead::rand_core::{OsRng, RngCore};
use axum::http::HeaderMap;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{LazyLock, Mutex, RwLock};
use std::time::{Duration, Instant};

use crate::crypto;
use crate::gateway::broker;
use crate::subscription_dispatch::{engines, reauth};
use crate::types::{ModelRequest, ModelResponse};

// Unix-seconds of the last CLI self-heal, to debounce concurrent triggers.
static LAST_CLI_SELF_HEAL: AtomicI64 = AtomicI64::new(0);

fn is_permanent_auth_failure(err: &str) -> bool {
    let e = err.to_lowercase();
    e.contains("refresh token was revoked")
        || e.contains("access token could not be refreshed")
        || e.contains("invalid_grant")
}

fn is_auth_failure(err: &str) -> bool {
    let e = err.to_lowercase();
    e.contains("invalid authentication")
        || e.contains("authentication_error")
        || e.contains("failed to authenticate")
        || e.contains("401")
        || e.contains("oauth")
        || is_permanent_auth_failure(err)
}

async fn mark_subscription_revoked(sub_id: &str) {
    crate::journal::retire(sub_id);
}

// When every donated subscription for a provider fails specifically with an
// AUTH error (not a rate/quota limit), the most likely cause is a stale,
// build-time-baked CLI whose auth broke after an upstream update (as Anthropic's
// did 2026-05-27). Reactively re-pull the latest CLI in the background so the
// NEXT request uses a current binary — no manual rebuild, no waiting on the
// periodic refresh. Debounced to at most once per 10 minutes.
fn maybe_self_heal_cli(provider: &str, err: &str) {
    let is_auth_failure = err.contains("Invalid authentication")
        || err.contains("authentication_error")
        || err.contains("invalid_grant")
        || err.contains("OAuth");
    if !is_auth_failure {
        return;
    }
    if provider == "codex" {
        // Codex is pinned to the build-time global install because it needs a
        // matching platform-native package. Runtime refresh can break that
        // pairing, so Codex updates must go through an image rebuild.
        return;
    }
    let pkg = match provider {
        "claude_code" => "@anthropic-ai/claude-code@latest",
        "codex" => "@openai/codex@latest",
        "opencode" => "opencode-ai@latest",
        _ => return,
    };
    let now = chrono::Utc::now().timestamp();
    let last = LAST_CLI_SELF_HEAL.load(Ordering::Relaxed);
    if now - last < 600 {
        return;
    }
    if LAST_CLI_SELF_HEAL
        .compare_exchange(last, now, Ordering::Relaxed, Ordering::Relaxed)
        .is_err()
    {
        return; // another request already kicked the heal
    }
    eprintln!("[router] all '{provider}' subs failed auth; self-healing CLI: npm install -g {pkg}");
    std::thread::spawn(move || {
        let status = std::process::Command::new("npm")
            .args(["install", "-g", pkg, "--no-fund", "--no-audit"])
            .status();
        eprintln!("[router] CLI self-heal for {pkg} finished: {status:?}");
    });
}

const CODEX_MODEL_CACHE_TTL: Duration = Duration::from_secs(300);

struct CachedCodexModels {
    fetched: Instant,
    models: Vec<engines::CodexModel>,
}

static CODEX_MODEL_CACHE: LazyLock<Mutex<HashMap<String, CachedCodexModels>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
static DISCOVERED_CODEX_MODELS: LazyLock<RwLock<HashSet<String>>> =
    LazyLock::new(|| RwLock::new(HashSet::new()));

pub const SUBSCRIPTION_MODELS: &[(&str, &str)] = &[
    ("claude-code-subscription", "claude_code"),
    ("claude-opus-4-7", "claude_code"),
    ("codex-subscription", "codex"),
    ("kimi-subscription", "kimi"),
    ("opencode-subscription", "opencode"),
];

pub fn is_subscription_model(model: &str) -> bool {
    provider_for(model).is_some()
}

pub fn is_vision_capable_subscription_model(model: &str) -> bool {
    matches!(provider_for(model), Some("claude_code"))
}

fn is_codex_model(model: &str) -> bool {
    DISCOVERED_CODEX_MODELS
        .read()
        .is_ok_and(|models| models.contains(model))
}

fn register_codex_models(models: &[engines::CodexModel]) {
    if let Ok(mut discovered) = DISCOVERED_CODEX_MODELS.write() {
        discovered.extend(models.iter().map(|model| model.id.clone()));
    }
}

pub(crate) fn provider_for(model: &str) -> Option<&'static str> {
    SUBSCRIPTION_MODELS
        .iter()
        .find(|(candidate, _)| *candidate == model)
        .map(|(_, provider)| *provider)
        .or_else(|| is_codex_model(model).then_some("codex"))
}

fn canonical_model_for_provider(provider: &str) -> Option<&'static str> {
    SUBSCRIPTION_MODELS
        .iter()
        .find(|(_, p)| *p == provider)
        .map(|(m, _)| *m)
}

fn canonical_provider_id(provider: &str) -> Option<&'static str> {
    match provider
        .trim()
        .to_ascii_lowercase()
        .replace('-', "_")
        .as_str()
    {
        "anthropic" | "claude" | "claude_code" => Some("claude_code"),
        "chatgpt" | "codex" | "openai" => Some("codex"),
        "kimi" | "kimi_code" | "moonshot" => Some("kimi"),
        "opencode" => Some("opencode"),
        _ => None,
    }
}

pub(crate) fn subscription_model_for_provider(provider: &str) -> Option<&'static str> {
    canonical_provider_id(provider).and_then(canonical_model_for_provider)
}

pub async fn codex_models_for_agent(agent_id: &str) -> Result<Vec<engines::CodexModel>, String> {
    let entries = broker::list_subscriptions(agent_id)
        .await
        .into_iter()
        .filter(|entry| {
            canonical_provider_id(&entry.provider) == Some("codex")
                && entry.status == "active"
                && !crate::journal::is_retired(&entry.id)
        })
        .collect::<Vec<_>>();
    if entries.is_empty() {
        return Err("no active Codex subscription for catalog agent".into());
    }

    let mut models_by_id = HashMap::new();
    let mut failures = Vec::new();
    for entry in entries {
        let cached = CODEX_MODEL_CACHE.lock().ok().and_then(|cache| {
            cache
                .get(&entry.id)
                .filter(|item| item.fetched.elapsed() < CODEX_MODEL_CACHE_TTL)
                .map(|item| item.models.clone())
        });
        let models = if let Some(cached) = cached {
            cached
        } else {
            let secret = match broker::subscription_credential(&entry.id, "codex").await {
                Some(secret) => secret,
                None => {
                    failures.push(format!("{}: credential unavailable", entry.id));
                    continue;
                }
            };
            let token_json = match secret.expose_utf8() {
                Ok(token_json) => token_json,
                Err(_) => {
                    failures.push(format!("{}: credential is not UTF-8", entry.id));
                    continue;
                }
            };
            let discovered = match engines::list_codex_models(token_json).await {
                Ok(models) => models,
                Err(error) => {
                    failures.push(format!("{}: {error}", entry.id));
                    continue;
                }
            };
            if let Ok(mut cache) = CODEX_MODEL_CACHE.lock() {
                cache.insert(
                    entry.id.clone(),
                    CachedCodexModels {
                        fetched: Instant::now(),
                        models: discovered.clone(),
                    },
                );
            }
            discovered
        };
        register_codex_models(&models);
        for model in models {
            models_by_id.entry(model.id.clone()).or_insert(model);
        }
    }

    if models_by_id.is_empty() {
        return Err(format!(
            "could not discover Codex subscription models: {}",
            failures.join("; ")
        ));
    }
    let mut models = models_by_id.into_values().collect::<Vec<_>>();
    models.sort_by(|left, right| left.id.cmp(&right.id));
    Ok(models)
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
        if canonical_provider_id(&target.provider_id) != Some(provider) {
            return Err(format!(
                "billingTarget provider '{}' does not match model provider '{provider}'",
                target.provider_id
            ));
        }
    }
    Ok(entries
        .into_iter()
        .filter(|entry| {
            canonical_provider_id(&entry.provider) == Some(provider)
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

fn shuffle_models(models: &mut [String]) {
    if models.len() < 2 {
        return;
    }
    for i in (1..models.len()).rev() {
        let j = (OsRng.next_u64() as usize) % (i + 1);
        models.swap(i, j);
    }
}

async fn any_subscription_models(
    headers: &HeaderMap,
    raw_body: &[u8],
) -> Result<Vec<String>, String> {
    let agent_id = authenticate_agent(headers, raw_body).await?;
    active_supported_models_for_agent(&agent_id).await
}

async fn any_vision_capable_subscription_models(
    headers: &HeaderMap,
    raw_body: &[u8],
) -> Result<Vec<String>, String> {
    let agent_id = authenticate_agent(headers, raw_body).await?;
    active_vision_capable_models_for_agent(&agent_id).await
}

pub async fn active_supported_models_for_agent(agent_id: &str) -> Result<Vec<String>, String> {
    let entries = broker::list_subscriptions(agent_id).await;
    let mut models = Vec::new();
    for entry in entries {
        if entry.status != "active" || crate::journal::is_retired(&entry.id) {
            continue;
        }
        let Some(model) = canonical_model_for_provider(&entry.provider) else {
            continue;
        };
        if !models.iter().any(|existing| existing == model) {
            models.push(model.to_string());
        }
    }
    if models.is_empty() {
        return Err("no active supported CLI subscription for signed agent".into());
    }
    shuffle_models(&mut models);
    Ok(models)
}

pub async fn active_vision_capable_models_for_agent(agent_id: &str) -> Result<Vec<String>, String> {
    let models = active_supported_models_for_agent(agent_id).await?;
    let mut vision_models = models
        .into_iter()
        .filter(|model| is_vision_capable_subscription_model(model))
        .collect::<Vec<_>>();
    if vision_models.is_empty() {
        return Err("no active vision-capable subscription model for signed agent".into());
    }
    shuffle_models(&mut vision_models);
    Ok(vision_models)
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
    let mut ordered = Vec::new();
    let mut idx = 0;
    while idx < scored.len() {
        let score = scored[idx].1;
        let mut group = Vec::new();
        while idx < scored.len() && (scored[idx].1 - score).abs() < f64::EPSILON {
            group.push(scored[idx].0.clone());
            idx += 1;
        }
        shuffle_models(&mut group);
        ordered.extend(group);
    }
    Ok(ordered)
}

/// `model: "any"` means: pick a random supported subscription model for the
/// signed agent, try it, and continue through the randomized candidate list
/// until one actually returns a successful response.
pub async fn dispatch_any_subscription(
    headers: &HeaderMap,
    request: &ModelRequest,
    raw_body: &[u8],
) -> ModelResponse {
    let models = match any_subscription_models(headers, raw_body).await {
        Ok(models) => models,
        Err(e) => return ModelResponse::failure(&request.model, e),
    };
    let mut errors = Vec::new();
    for model in models {
        let mut candidate = request.clone();
        candidate.model = model.clone();
        let resp = dispatch_subscription(headers, &candidate, raw_body).await;
        if resp.success {
            return resp;
        }
        errors.push(format!(
            "{}: {}",
            model,
            resp.error.unwrap_or_else(|| "failed".to_string())
        ));
    }
    ModelResponse::failure(
        &request.model,
        format!(
            "no working subscription model for signed agent; tried {}",
            errors.join("; ")
        ),
    )
}

/// `model: "any-vision-capable"` means: pick a random active subscription
/// model that can read image references through the router's current CLI path.
pub async fn dispatch_any_vision_capable_subscription(
    headers: &HeaderMap,
    request: &ModelRequest,
    raw_body: &[u8],
) -> ModelResponse {
    let models = match any_vision_capable_subscription_models(headers, raw_body).await {
        Ok(models) => models,
        Err(e) => return ModelResponse::failure(&request.model, e),
    };
    let mut errors = Vec::new();
    for model in models {
        let mut candidate = request.clone();
        candidate.model = model.clone();
        let resp = dispatch_subscription(headers, &candidate, raw_body).await;
        if resp.success {
            return resp;
        }
        errors.push(format!(
            "{}: {}",
            model,
            resp.error.unwrap_or_else(|| "failed".to_string())
        ));
    }
    ModelResponse::failure(
        &request.model,
        format!(
            "no working vision-capable subscription model for signed agent; tried {}",
            errors.join("; ")
        ),
    )
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
    let mut errors = Vec::new();
    for model in models {
        let mut candidate = request.clone();
        candidate.model = model.clone();
        let resp = dispatch_subscription(headers, &candidate, raw_body).await;
        if resp.success {
            return resp;
        }
        errors.push(format!(
            "{}: {}",
            model,
            resp.error.unwrap_or_else(|| "failed".to_string())
        ));
    }
    ModelResponse::failure(
        &request.model,
        format!(
            "no working quality-ranked model for task '{task}'; tried {}",
            errors.join("; ")
        ),
    )
}

pub async fn dispatch_subscription_for_agent(
    agent_id: &str,
    request: &ModelRequest,
) -> ModelResponse {
    let provider = match provider_for(&request.model) {
        Some(p) => p,
        None => return ModelResponse::failure(&request.model, "unknown subscription model".into()),
    };
    dispatch_subscription_attempt(provider, agent_id, request)
        .await
        .response
}

/// Handle a chat completions request whose model name is a CLI-backed
/// subscription. The caller (agent) must supply the HMAC header trio so
/// we can scope decryption to the agent that donated the subscription.
pub async fn dispatch_subscription(
    headers: &HeaderMap,
    request: &ModelRequest,
    raw_body: &[u8],
) -> ModelResponse {
    let provider = match provider_for(&request.model) {
        Some(p) => p,
        None => return ModelResponse::failure(&request.model, "unknown subscription model".into()),
    };

    let agent_id = match authenticate_agent(headers, raw_body).await {
        Ok(id) => id,
        Err(e) => return ModelResponse::failure(&request.model, e),
    };
    let skip_weles_reauth = headers
        .get("x-brama-skip-weles-reauth")
        .and_then(|v| v.to_str().ok())
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);

    let mut tried_weles_reauth = false;
    loop {
        let attempt = dispatch_subscription_attempt(provider, &agent_id, request).await;
        if attempt.response.success {
            return attempt.response;
        }

        // Prefer the classified auth-failure candidate. But a stale/expired
        // subscription can also surface as an UNCLASSIFIED failure — e.g. the
        // codex CLI exits 1 with no "401"/"oauth" token in stderr. For providers
        // Weles can reauth on the host, attempt one broker-driven reauth on any
        // failure instead of returning a hard error and letting the token die
        // silently (the exact gap that stranded codex). Skip when already tried
        // or the caller opted out.
        let candidate = match attempt.reauth_candidate {
            Some(c) => c,
            None => {
                if tried_weles_reauth
                    || skip_weles_reauth
                    || !reauth::provider_is_reauthable(provider)
                {
                    return attempt.response;
                }
                ReauthCandidate {
                    subscription_id: String::new(),
                    error: attempt
                        .response
                        .error
                        .clone()
                        .unwrap_or_else(|| "unclassified subscription failure".to_string()),
                }
            }
        };

        if skip_weles_reauth {
            return attempt.response;
        }
        if tried_weles_reauth {
            maybe_self_heal_cli(provider, &candidate.error);
            return attempt.response;
        }
        tried_weles_reauth = true;

        eprintln!(
            "[router] auth failure for {provider} sub {}; requesting Weles reauth",
            candidate.subscription_id
        );
        match reauth::reauth_provider(
            &agent_id,
            provider,
            &candidate.subscription_id,
            &request.model,
            &candidate.error,
        )
        .await
        {
            Ok(result) if result.refreshed => {
                eprintln!(
                    "[router] Weles reauth refreshed {provider} via {}; retrying dispatch",
                    result.source
                );
                continue;
            }
            Ok(result) => {
                eprintln!(
                    "[router] Weles reauth for {provider} returned refreshed=false via {}",
                    result.source
                );
                maybe_self_heal_cli(provider, &candidate.error);
                return attempt.response;
            }
            Err(e) => {
                eprintln!("[router] Weles reauth for {provider} failed: {e}");
                maybe_self_heal_cli(provider, &candidate.error);
                return attempt.response;
            }
        }
    }
}

struct ReauthCandidate {
    subscription_id: String,
    error: String,
}

struct AttemptOutcome {
    response: ModelResponse,
    reauth_candidate: Option<ReauthCandidate>,
}

async fn run_provider_with_token(
    provider: &str,
    request: &ModelRequest,
    agent_id: &str,
    subscription_id: &str,
    token: &str,
) -> ModelResponse {
    match provider {
        "claude_code" => engines::run_claude_code(request, agent_id, subscription_id, token).await,
        "codex" => engines::run_codex(request, agent_id, subscription_id, token).await,
        "kimi" => engines::run_kimi(request, agent_id, subscription_id, token).await,
        "opencode" => engines::run_opencode(request, agent_id, subscription_id, token).await,
        _ => ModelResponse::failure(&request.model, "unreachable".into()),
    }
}

async fn dispatch_subscription_attempt(
    provider: &str,
    agent_id: &str,
    request: &ModelRequest,
) -> AttemptOutcome {
    let rows = match eligible_subscription_entries(
        broker::list_subscriptions(agent_id).await,
        provider,
        request.billing_target.as_ref(),
    ) {
        Ok(rows) => rows,
        Err(error) => {
            return AttemptOutcome {
                response: ModelResponse::failure(&request.model, error),
                reauth_candidate: None,
            }
        }
    };
    if rows.is_empty() {
        return AttemptOutcome {
            response: ModelResponse::failure(
                &request.model,
                request.billing_target.as_ref().map_or_else(
                    || format!("no active '{provider}' subscription for agent"),
                    |target| {
                        format!(
                            "selected subscription '{}' is not active for provider '{provider}' and agent",
                            target.subscription_id
                        )
                    },
                ),
            ),
            reauth_candidate: None,
        };
    }

    // Try each subscription in turn. If a call returns a per-subscription
    // exhaustion signal (per-window quota, auth-expired OAuth, 401), rotate
    // to the next sub. Anything else surfaces immediately so we don't mask
    // upstream API breakage as exhaustion.
    let mut last_failure: Option<ModelResponse> = None;
    let mut last_auth_failure: Option<ReauthCandidate> = None;
    for (idx, entry) in rows.iter().enumerate() {
        let sub_id = entry.id.clone();
        let token = match broker::subscription_credential(&sub_id, provider).await {
            Some(token) => token,
            None => {
                eprintln!("[router] sub {sub_id} has no capability credential (skipping)");
                continue;
            }
        };
        let token = match token.expose_utf8() {
            Ok(token) => token,
            Err(_) => {
                eprintln!("[router] sub {sub_id} capability credential is invalid (skipping)");
                continue;
            }
        };
        let result = run_provider_with_token(provider, request, agent_id, &sub_id, token).await;
        if result.success {
            if idx > 0 {
                eprintln!(
                    "[router] rotated to sub {sub_id} (idx={idx}) for {provider} after prior burnouts"
                );
            }
            return AttemptOutcome {
                response: result,
                reauth_candidate: None,
            };
        }
        let err = result.error.clone().unwrap_or_default();
        if is_permanent_auth_failure(&err) {
            eprintln!("[router] sub {sub_id} permanent auth failure; revoking");
            mark_subscription_revoked(&sub_id).await;
        }
        if is_auth_failure(&err) {
            last_auth_failure = Some(ReauthCandidate {
                subscription_id: sub_id.clone(),
                error: err.clone(),
            });
        }
        let is_subscription_burnout = err.contains("hit your limit")
            || err.contains("hit your session limit")
            || err.contains("session limit")
            || err.contains("usage limit")
            || err.contains("weekly limit")
            || err.contains("authentication_error")
            || err.contains("Invalid authentication credentials")
            || err.contains("401")
            || err.contains("rate_limit")
            || is_permanent_auth_failure(&err);
        if !is_subscription_burnout {
            return AttemptOutcome {
                response: result,
                reauth_candidate: last_auth_failure,
            };
        }
        eprintln!("[router] sub {sub_id} burnt out ({err}); rotating to next");
        last_failure = Some(result);
    }
    // Every active sub failed. If the failures are auth errors (not quota), the
    // CLI itself is likely stale — kick a reactive background refresh so the next
    // request recovers automatically.
    let response = last_failure.unwrap_or_else(|| {
        ModelResponse::failure(
            &request.model,
            format!("all '{provider}' subs burnt out for agent"),
        )
    });
    AttemptOutcome {
        response,
        reauth_candidate: last_auth_failure,
    }
}

fn string_field(row: &Value, key: &str) -> String {
    match row.get(key) {
        Some(Value::String(value)) => value.trim().to_string(),
        Some(Value::Number(value)) => value.to_string(),
        Some(Value::Bool(value)) => value.to_string(),
        _ => String::new(),
    }
}
