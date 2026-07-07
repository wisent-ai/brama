//! Route `"*-subscription"` chat completions requests to the matching CLI.

use std::collections::HashMap;
use std::env;
use std::sync::atomic::{AtomicI64, Ordering};

use aes_gcm::aead::rand_core::{OsRng, RngCore};
use axum::http::HeaderMap;
use serde_json::Value;

use crate::crypto;
use crate::gateway::supabase;
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
    let Ok(client) = supabase::client() else {
        return;
    };
    let body = serde_json::json!({
        "status": "revoked",
        "updated_at": chrono::Utc::now().to_rfc3339(),
    });
    let _ = client
        .from("trade_agent_subscriptions")
        .eq("id", sub_id)
        .update(body.to_string())
        .execute()
        .await;
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

pub const SUBSCRIPTION_MODELS: &[(&str, &str)] = &[
    ("claude-code-subscription", "claude_code"),
    ("claude-opus-4-7", "claude_code"),
    ("codex-subscription", "codex"),
    ("kimi-subscription", "kimi"),
    ("opencode-subscription", "opencode"),
];

pub fn is_subscription_model(model: &str) -> bool {
    SUBSCRIPTION_MODELS.iter().any(|(m, _)| *m == model)
}

pub fn is_vision_capable_subscription_model(model: &str) -> bool {
    matches!(provider_for(model), Some("claude_code"))
}

pub(crate) fn provider_for(model: &str) -> Option<&'static str> {
    SUBSCRIPTION_MODELS
        .iter()
        .find(|(m, _)| *m == model)
        .map(|(_, p)| *p)
}

fn canonical_model_for_provider(provider: &str) -> Option<&'static str> {
    SUBSCRIPTION_MODELS
        .iter()
        .find(|(_, p)| *p == provider)
        .map(|(m, _)| *m)
}

async fn authenticate_agent(headers: &HeaderMap, raw_body: &[u8]) -> Result<String, String> {
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

    let client = supabase::client().map_err(|e| e.to_string())?;

    // Per-agent auth secret, with the master AGENT_AUTH_SECRET env as the
    // shared-secret path the TS impl also uses.
    let secret = match supabase::get_agent_auth_secret(&client, &agent_id).await {
        Ok(Some(s)) => s,
        Ok(None) | Err(_) => match env::var("AGENT_AUTH_SECRET") {
            Ok(s) => s,
            Err(_) => {
                return Err(
                    "no auth secret for agent, and AGENT_AUTH_SECRET unset".into(),
                );
            }
        },
    };

    let headers_for_check = crypto::HmacHeaders {
        agent_id: &agent_id,
        timestamp: ts,
        signature: sig,
    };
    crypto::verify_agent_hmac(&headers_for_check, raw_body, secret.as_bytes())
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
    let client = supabase::client().map_err(|e| e.to_string())?;
    let resp = client
        .from("trade_agent_subscriptions")
        .select("provider")
        .eq("instance_id", &agent_id)
        .eq("status", "active")
        .execute()
        .await
        .map_err(|e| format!("supabase: {e}"))?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(format!(
            "supabase returned HTTP {} while loading active subscriptions",
            status.as_u16()
        ));
    }

    let rows: Vec<Value> = serde_json::from_str(&text).unwrap_or_default();
    if rows.is_empty() {
        return Err("no active subscription for signed agent".into());
    }
    let mut models = Vec::new();
    for row in rows {
        let provider = string_field(&row, "provider");
        let Some(model) = canonical_model_for_provider(&provider) else {
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
    row.get("metadata")
        .and_then(|metadata| metadata.get("score"))
        .and_then(|score| score.as_f64())
}

fn model_field(row: &Value) -> Option<String> {
    row.get("metadata")
        .and_then(|metadata| metadata.get("model"))
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

async fn task_quality_models(
    agent_id: &str,
    task: &str,
) -> Result<Vec<String>, String> {
    let active_models = active_supported_models_for_agent(agent_id).await?;
    let client = supabase::client().map_err(|e| e.to_string())?;
    let resp = client
        .from("subscription_router_checks")
        .select("status,metadata,checked_at")
        .eq("agent_id", agent_id)
        .eq("source", "model-router-task-quality")
        .eq("account_identifier", task)
        .execute()
        .await
        .map_err(|e| format!("load task quality checks: {e}"))?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if status.as_u16() == 404 || text.contains("subscription_router_checks") {
        return Err(format!("no quality checks configured for task '{task}'"));
    }
    if !status.is_success() {
        return Err(format!(
            "load task quality checks: http_{} {text}",
            status.as_u16()
        ));
    }
    let rows = serde_json::from_str::<Vec<Value>>(&text)
        .map_err(|e| format!("parse task quality checks: {e}"))?;
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
        .get("x-model-router-skip-weles-reauth")
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
                if tried_weles_reauth || skip_weles_reauth || !reauth::provider_is_reauthable(provider) {
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

async fn dispatch_subscription_attempt(
    provider: &str,
    agent_id: &str,
    request: &ModelRequest,
) -> AttemptOutcome {
    let client = match supabase::client() {
        Ok(c) => c,
        Err(e) => {
            return AttemptOutcome {
                response: ModelResponse::failure(&request.model, e.to_string()),
                reauth_candidate: None,
            }
        }
    };

    // Look up ALL active donated subscriptions for this agent + provider.
    // Order by created_at ascending so the rotation is deterministic and the
    // oldest sub gets first crack — a freshly-donated sub is held in reserve
    // until older ones have burnt their per-window quota.
    let resp = client
        .from("trade_agent_subscriptions")
        .select("id,key_encrypted,status,created_at")
        .eq("instance_id", agent_id)
        .eq("provider", provider)
        .eq("status", "active")
        .order("created_at.asc")
        .execute()
        .await;
    let rows: Vec<Value> = match resp {
        Ok(r) => {
            let t = r.text().await.unwrap_or_default();
            serde_json::from_str(&t).unwrap_or_default()
        }
        Err(e) => {
            return AttemptOutcome {
                response: ModelResponse::failure(&request.model, format!("supabase: {e}")),
                reauth_candidate: None,
            }
        }
    };
    if rows.is_empty() {
        return AttemptOutcome {
            response: ModelResponse::failure(
                &request.model,
                format!("no active '{provider}' subscription for agent"),
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
    for (idx, row) in rows.iter().enumerate() {
        let encrypted = match row.get("key_encrypted").and_then(|v: &Value| v.as_str()) {
            Some(s) => s.to_string(),
            None => continue,
        };
        let sub_id = row
            .get("id")
            .and_then(|v: &Value| v.as_str())
            .unwrap_or("?")
            .to_string();
        let token = match crypto::decrypt(&encrypted) {
            Ok(t) => t,
            Err(e) => {
                eprintln!("[router] sub {sub_id} decrypt failed (skipping): {e}");
                continue;
            }
        };
        let result = match provider {
            "claude_code" => engines::run_claude_code(request, agent_id, &sub_id, &token).await,
            "codex" => engines::run_codex(request, agent_id, &sub_id, &token).await,
            "kimi" => engines::run_kimi(request, agent_id, &sub_id, &token).await,
            "opencode" => engines::run_opencode(request, agent_id, &sub_id, &token).await,
            _ => ModelResponse::failure(&request.model, "unreachable".into()),
        };
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
