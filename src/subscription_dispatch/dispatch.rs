//! Route `"*-subscription"` chat completions requests to the matching CLI.

use std::env;

use axum::http::HeaderMap;
use serde_json::Value;

use crate::crypto;
use crate::gateway::supabase;
use crate::subscription_dispatch::engines;
use crate::types::{ModelRequest, ModelResponse};

pub const SUBSCRIPTION_MODELS: &[(&str, &str)] = &[
    ("claude-code-subscription", "claude_code"),
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

    // Look up the donated subscription for this agent + provider.
    let resp = client
        .from("trade_agent_subscriptions")
        .select("key_encrypted,status")
        .eq("instance_id", agent_id)
        .eq("provider", provider)
        .eq("status", "active")
        .execute()
        .await;
    let rows: Vec<Value> = match resp {
        Ok(r) => {
            let t = r.text().await.unwrap_or_default();
            serde_json::from_str(&t).unwrap_or_default()
        }
        Err(e) => return ModelResponse::failure(&request.model, format!("supabase: {e}")),
    };
    let encrypted = match rows.first().and_then(|r| r.get("key_encrypted")).and_then(|v: &Value| v.as_str())
    {
        Some(s) => s.to_string(),
        None => {
            return ModelResponse::failure(
                &request.model,
                format!("no active '{provider}' subscription for agent"),
            );
        }
    };
    let token = match crypto::decrypt(&encrypted) {
        Ok(t) => t,
        Err(e) => return ModelResponse::failure(&request.model, format!("decrypt: {e}")),
    };

    match provider {
        "claude_code" => engines::run_claude_code(request, &token).await,
        "codex" => engines::run_codex(request, &token).await,
        "kimi" => engines::run_kimi(request, &token).await,
        "opencode" => engines::run_opencode(request, &token).await,
        _ => ModelResponse::failure(&request.model, "unreachable".into()),
    }
}
