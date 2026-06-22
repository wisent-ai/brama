//! Route `"*-subscription"` chat completions requests to the matching CLI.

use std::env;
use std::sync::atomic::{AtomicI64, Ordering};

use axum::http::HeaderMap;
use serde_json::Value;

use crate::crypto;
use crate::gateway::supabase;
use crate::subscription_dispatch::engines;
use crate::types::{ModelRequest, ModelResponse};

// Unix-seconds of the last CLI self-heal, to debounce concurrent triggers.
static LAST_CLI_SELF_HEAL: AtomicI64 = AtomicI64::new(0);

fn is_permanent_auth_failure(err: &str) -> bool {
    let e = err.to_lowercase();
    e.contains("refresh token was revoked")
        || e.contains("access token could not be refreshed")
        || e.contains("invalid_grant")
}

async fn mark_subscription_revoked(sub_id: &str) {
    let Ok(client) = supabase::client() else { return };
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

fn provider_for(model: &str) -> Option<&'static str> {
    SUBSCRIPTION_MODELS
        .iter()
        .find(|(m, _)| *m == model)
        .map(|(_, p)| *p)
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

    let agent_id = match headers.get("x-agent-id").and_then(|v| v.to_str().ok()) {
        Some(s) if !s.is_empty() => s,
        _ => return ModelResponse::failure(&request.model, "missing x-agent-id header".into()),
    };
    let ts = headers
        .get("x-agent-timestamp")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let sig = headers
        .get("x-agent-signature")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    let client = match supabase::client() {
        Ok(c) => c,
        Err(e) => return ModelResponse::failure(&request.model, e.to_string()),
    };

    // Per-agent auth secret, with the master AGENT_AUTH_SECRET env as the
    // shared-secret path the TS impl also uses.
    let secret = match supabase::get_agent_auth_secret(&client, agent_id).await {
        Ok(Some(s)) => s,
        Ok(None) | Err(_) => match env::var("AGENT_AUTH_SECRET") {
            Ok(s) => s,
            Err(_) => {
                return ModelResponse::failure(
                    &request.model,
                    "no auth secret for agent, and AGENT_AUTH_SECRET unset".into(),
                );
            }
        },
    };

    let headers_for_check = crypto::HmacHeaders {
        agent_id,
        timestamp: ts,
        signature: sig,
    };
    if let Err(e) =
        crypto::verify_agent_hmac(&headers_for_check, raw_body, secret.as_bytes())
    {
        return ModelResponse::failure(&request.model, format!("auth: {e}"));
    }

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
        Err(e) => return ModelResponse::failure(&request.model, format!("supabase: {e}")),
    };
    if rows.is_empty() {
        return ModelResponse::failure(
            &request.model,
            format!("no active '{provider}' subscription for agent"),
        );
    }

    // Try each subscription in turn. If a call returns a per-subscription
    // exhaustion signal (per-window quota, auth-expired OAuth, 401), rotate
    // to the next sub. Anything else surfaces immediately so we don't mask
    // upstream API breakage as exhaustion.
    let mut last_failure: Option<ModelResponse> = None;
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
                eprintln!(
                    "[router] sub {sub_id} decrypt failed (skipping): {e}"
                );
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
            return result;
        }
        let err = result.error.clone().unwrap_or_default();
        if is_permanent_auth_failure(&err) {
            eprintln!("[router] sub {sub_id} permanent auth failure; revoking");
            mark_subscription_revoked(&sub_id).await;
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
            return result;
        }
        eprintln!(
            "[router] sub {sub_id} burnt out ({err}); rotating to next"
        );
        last_failure = Some(result);
    }
    // Every active sub failed. If the failures are auth errors (not quota), the
    // CLI itself is likely stale — kick a reactive background refresh so the next
    // request recovers automatically.
    if let Some(ref resp) = last_failure {
        if let Some(err) = resp.error.as_deref() {
            maybe_self_heal_cli(provider, err);
        }
    }
    last_failure.unwrap_or_else(|| {
        ModelResponse::failure(
            &request.model,
            format!("all '{provider}' subs burnt out for agent"),
        )
    })
}
