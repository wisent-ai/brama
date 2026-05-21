//! The four CLI engine specializations. Each takes a decrypted donated
//! token plus a prompt and returns a `ModelResponse`. Which credential
//! location each engine expects:
//!
//! - `claude_code`: `CLAUDE_CODE_OAUTH_TOKEN` env var (no file needed).
//! - `codex`: `$HOME/.codex/auth.json` containing the JSON auth blob.
//! - `kimi`: `$HOME/.kimi/credentials/kimi-code.json` with the token JSON.
//! - `opencode`: reads `CLAUDE_CODE_OAUTH_TOKEN` in practice when backed by
//!   a Claude subscription; we pass it through as env.

use std::collections::HashMap;
use std::path::Path;

use tokio::fs;

use crate::crypto;
use crate::gateway::supabase;
use crate::subscription_dispatch::runtime::{
    build_prompt_from, run_cli, Sandbox,
};
use crate::types::{ModelRequest, ModelResponse};

/// If the CLI run rotated the donated OAuth blob in-sandbox, re-encrypt
/// and push it back to the specific `trade_agent_subscriptions` row that
/// the dispatcher selected for this request. Scoping the UPDATE by
/// subscription_id is critical now that the dispatcher iterates a pool
/// of multiple active rows per (instance_id, provider) — an unscoped
/// update would smear sub A's refreshed key onto sub B and C, collapsing
/// the pool to a single value. Best-effort: persistence failures do NOT
/// fail the caller's request.
async fn persist_refreshed_token(
    subscription_id: &str,
    provider: &str,
    agent_id: &str,
    original: &str,
    creds_path: &Path,
) {
    let after = match fs::read_to_string(creds_path).await {
        Ok(s) => s,
        Err(_) => return,
    };
    if after.trim() == original.trim() {
        return;
    }
    let client = match supabase::client() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[router] refresh persist: supabase client: {e}");
            return;
        }
    };
    let encrypted = match crypto::encrypt(&after) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("[router] refresh persist: encrypt: {e}");
            return;
        }
    };
    let body = serde_json::json!({ "key_encrypted": encrypted }).to_string();
    let resp = client
        .from("trade_agent_subscriptions")
        .eq("id", subscription_id)
        .update(body)
        .execute()
        .await;
    match resp {
        Ok(r) if r.status().is_success() => {
            eprintln!(
                "[router] refresh persist: rotated {provider} token for sub {subscription_id} (agent {agent_id})"
            );
        }
        Ok(r) => {
            let t = r.text().await.unwrap_or_default();
            eprintln!("[router] refresh persist: update non-2xx: {t}");
        }
        Err(e) => {
            eprintln!("[router] refresh persist: update: {e}");
        }
    }
}

pub async fn run_claude_code(
    request: &ModelRequest,
    agent_id: &str,
    subscription_id: &str,
    token: &str,
) -> ModelResponse {
    let sandbox = match Sandbox::new() {
        Ok(s) => s,
        Err(e) => return ModelResponse::failure(&request.model, e.to_string()),
    };
    // Claude Code 2.x authenticates from $HOME/.claude/.credentials.json. It
    // does NOT read CLAUDE_CODE_OAUTH_TOKEN on its own for the inference API
    // call, even though older docs suggest it — you get a bare 401 if that's
    // all you provide. `token` is either (a) a JSON blob matching the
    // claudeAiOauth structure (preferred — the donor's full OAuth record) or
    // (b) a plain "sk-ant-oat01-..." access token string which we wrap into
    // the minimum shape v2 accepts.
    let creds_json = materialize_credentials(token);
    let claude_dir = sandbox.home.join(".claude");
    if let Err(e) = fs::create_dir_all(&claude_dir).await {
        return ModelResponse::failure(&request.model, format!("mkdir .claude: {e}"));
    }
    let creds_path = claude_dir.join(".credentials.json");
    if let Err(e) = fs::write(&creds_path, &creds_json).await {
        return ModelResponse::failure(&request.model, format!("write creds: {e}"));
    }
    let _ = chmod_private(&creds_path).await;
    // Claude Code CLI has its own built-in software-engineering system
    // prompt. If we fold our caller's system prompt into the `claude -p`
    // text, Claude's default persona overrides ours and it refuses
    // non-coding tasks ("I'm a software engineering assistant..."). Use
    // --append-system-prompt so the caller's instructions sit alongside
    // Claude's default and actually get respected.
    let user_prompt = build_prompt_from(&None, &request.messages);
    let system_prompt = request.system.clone().unwrap_or_default();
    let mut env = HashMap::new();
    env.insert("CLAUDE_CODE_OAUTH_TOKEN".into(), token.to_string());
    // The router runs in an ephemeral per-request sandbox with no human
    // operator to approve tool prompts. Without bypass, every Read /
    // WebFetch returns "I wasn't granted permission" and any image / file
    // / URL reference in the prompt is dropped silently. We can't use
    // --dangerously-skip-permissions (Claude CLI refuses it under root,
    // which the Cloud Run container runs as) and we can't use the
    // variadic --allowed-tools (it consumes the prompt that follows it).
    // --permission-mode bypassPermissions is a single-value flag with the
    // same effect, with no root check.
    let mut argv: Vec<&str> = vec![
        "claude", "-p",
        "--permission-mode", "bypassPermissions",
    ];
    if !system_prompt.is_empty() {
        argv.push("--append-system-prompt");
        argv.push(&system_prompt);
    }
    argv.push(&user_prompt);
    let response = run_cli(&request.model, &argv, &sandbox, env).await;
    // Claude CLI auto-refreshes the OAuth token in-place when the access
    // token is expired but the refresh token is still valid. Capture any
    // rotation and push it back to the DB so the next dispatch doesn't
    // start from a stale blob. Compare against what we wrote (creds_json)
    // rather than the bare `token` the dispatcher decrypted — the sandbox
    // file is always in the wrapped format.
    persist_refreshed_token(subscription_id, "claude_code", agent_id, &creds_json, &creds_path).await;
    response
}

fn materialize_credentials(token: &str) -> String {
    // If caller already stored the full {"claudeAiOauth":{...}} blob, pass it
    // through verbatim. Otherwise wrap a bare access token in the minimum
    // structure v2 accepts: scopes including user:inference + an expiresAt
    // 24h in the future so the CLI's local check passes (a token that's
    // actually expired will still get rejected at the API).
    let trimmed = token.trim();
    if trimmed.starts_with('{') {
        return trimmed.to_string();
    }
    let expires_ms = chrono::Utc::now().timestamp_millis() + 24 * 60 * 60 * 1000;
    serde_json::json!({
        "claudeAiOauth": {
            "accessToken": trimmed,
            "refreshToken": "",
            "expiresAt": expires_ms,
            "scopes": [
                "user:file_upload",
                "user:inference",
                "user:mcp_servers",
                "user:profile",
                "user:sessions:claude_code"
            ],
            "subscriptionType": "max"
        }
    })
    .to_string()
}

pub async fn run_codex(
    request: &ModelRequest,
    agent_id: &str,
    subscription_id: &str,
    token_json: &str,
) -> ModelResponse {
    let sandbox = match Sandbox::new() {
        Ok(s) => s,
        Err(e) => return ModelResponse::failure(&request.model, e.to_string()),
    };
    let codex_dir = sandbox.home.join(".codex");
    if let Err(e) = fs::create_dir_all(&codex_dir).await {
        return ModelResponse::failure(&request.model, format!("mkdir: {e}"));
    }
    let auth_path = codex_dir.join("auth.json");
    if let Err(e) = fs::write(&auth_path, token_json).await {
        return ModelResponse::failure(&request.model, format!("write auth.json: {e}"));
    }
    let prompt = build_prompt_from(&request.system, &request.messages);
    let response = run_cli(
        &request.model,
        &[
            "codex",
            "exec",
            "--dangerously-bypass-approvals-and-sandbox",
            "-C",
            sandbox.home.to_str().unwrap_or("."),
            &prompt,
        ],
        &sandbox,
        HashMap::new(),
    )
    .await;
    persist_refreshed_token(subscription_id, "codex", agent_id, token_json, &auth_path).await;
    response
}

pub async fn run_kimi(
    request: &ModelRequest,
    agent_id: &str,
    subscription_id: &str,
    token_json: &str,
) -> ModelResponse {
    let sandbox = match Sandbox::new() {
        Ok(s) => s,
        Err(e) => return ModelResponse::failure(&request.model, e.to_string()),
    };
    let kimi_dir = sandbox.home.join(".kimi").join("credentials");
    if let Err(e) = fs::create_dir_all(&kimi_dir).await {
        return ModelResponse::failure(&request.model, format!("mkdir: {e}"));
    }
    let creds_path = kimi_dir.join("kimi-code.json");
    if let Err(e) = fs::write(&creds_path, token_json).await {
        return ModelResponse::failure(&request.model, format!("write kimi creds: {e}"));
    }
    let _ = chmod_private(&creds_path).await;
    let prompt = build_prompt_from(&request.system, &request.messages);
    let response = run_cli(
        &request.model,
        &[
            "kimi",
            "--print",
            "--yolo",
            "--work-dir",
            sandbox.home.to_str().unwrap_or("."),
            "-p",
            &prompt,
        ],
        &sandbox,
        HashMap::new(),
    )
    .await;
    persist_refreshed_token(subscription_id, "kimi", agent_id, token_json, &creds_path).await;
    response
}

pub async fn run_opencode(
    request: &ModelRequest,
    _agent_id: &str,
    _subscription_id: &str,
    token: &str,
) -> ModelResponse {
    let sandbox = match Sandbox::new() {
        Ok(s) => s,
        Err(e) => return ModelResponse::failure(&request.model, e.to_string()),
    };
    let prompt = build_prompt_from(&request.system, &request.messages);
    let mut env = HashMap::new();
    env.insert("CLAUDE_CODE_OAUTH_TOKEN".into(), token.to_string());
    run_cli(
        &request.model,
        &["opencode", "run", &prompt],
        &sandbox,
        env,
    )
    .await
}

async fn chmod_private(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o600);
        fs::set_permissions(path, perms).await?;
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Ok(())
}
