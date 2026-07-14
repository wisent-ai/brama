//! The four CLI engine specializations. Each takes a decrypted donated
//! token plus a prompt and returns a `ModelResponse`. Which credential
//! location each engine expects:
//!
//! - `claude_code`: `CLAUDE_CODE_OAUTH_TOKEN` env var (no file needed).
//! - `codex`: `$HOME/.codex/auth.json` containing the JSON auth blob.
//! - `kimi`: `$HOME/.kimi-code/credentials/kimi-code.json` with the OAuth token
//!   JSON and a matching `$HOME/.kimi-code/config.toml` that selects the
//!   `kimi-for-coding` model via the official Kimi provider.
//! - `opencode`: reads `CLAUDE_CODE_OAUTH_TOKEN` in practice when backed by
//!   a Claude subscription; we pass it through as env.

use std::collections::HashMap;
use std::path::Path;

use tokio::fs;

use crate::subscription_dispatch::runtime::{
    build_prompt_from, codex_coverage_hooks_enabled, materialize_codex_coverage_hooks, run_cli,
    Sandbox, CODEX_HOOK_COVERAGE_LOG,
};
use crate::types::{ModelRequest, ModelResponse};


pub async fn run_claude_code(
    request: &ModelRequest,
    _agent_id: &str,
    _subscription_id: &str,
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
    let mut user_prompt = build_prompt_from(&None, &request.messages);
    let img_files =
        crate::subscription_dispatch::runtime::materialize_images(&request.messages, &sandbox.home)
            .await;
    if !img_files.is_empty() {
        let refs = img_files
            .iter()
            .map(|f| format!("@{f}"))
            .collect::<Vec<_>>()
            .join(" ");
        user_prompt = format!("{user_prompt}\n\nRead these attached images first: {refs}");
    }
    let system_prompt = request.system.clone().unwrap_or_default();
    let mut env = HashMap::new();
    env.insert("CLAUDE_CODE_OAUTH_TOKEN".into(), token.to_string());
    // Pre-approve Read + WebFetch via --settings JSON blob (the only flag
    // path that doesn't trigger the root guard, doesn't eat the prompt,
    // and doesn't get denied). --dangerously-skip-permissions and
    // --permission-mode bypassPermissions both fail under root (Cloud
    // Run); --allowed-tools is variadic and consumes the prompt;
    // --permission-mode dontAsk denies instead of allowing.
    let settings_json = r#"{"permissions":{"allow":["Read","WebFetch"]}}"#;
    let mut argv: Vec<&str> = vec!["claude", "-p", "--settings", settings_json];
    if request.model.starts_with("claude-") && request.model != "claude-code-subscription" {
        argv.push("--model");
        argv.push(&request.model);
    }
    if !system_prompt.is_empty() {
        argv.push("--append-system-prompt");
        argv.push(&system_prompt);
    }
    argv.push(&user_prompt);
    let response = run_cli(&request.model, &argv, &sandbox, env).await;
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
    _agent_id: &str,
    _subscription_id: &str,
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
    if codex_coverage_hooks_enabled() {
        if let Err(e) = materialize_codex_coverage_hooks(&codex_dir).await {
            return ModelResponse::failure(&request.model, format!("materialize codex hooks: {e}"));
        }
    }
    let prompt = build_prompt_from(&request.system, &request.messages);
    // Cloud Run's npm global install has the platform-specific optional
    // dependency; runtime staged installs under /opt/cli-* do not reliably
    // pull it, so prefer the JS wrapper in the image. Local cargo-run smoke
    // tests don't have that image path, so allow an override and finally fall
    // back to the PATH-resolved `codex` binary.
    const DOCKER_CODEX_BIN: &str = "/usr/local/lib/node_modules/@openai/codex/bin/codex.js";
    let codex_bin = std::env::var("BRAMA_CODEX_BIN").unwrap_or_else(|_| {
        if Path::new(DOCKER_CODEX_BIN).exists() {
            DOCKER_CODEX_BIN.to_string()
        } else {
            "codex".to_string()
        }
    });
    let mut env = HashMap::new();
    env.insert(
        "CODEX_HOME".to_string(),
        codex_dir.to_str().unwrap_or("").to_string(),
    );
    let raw = run_cli(
        &request.model,
        &[
            codex_bin.as_str(),
            "exec",
            "--dangerously-bypass-approvals-and-sandbox",
            "--json",
            "--dangerously-bypass-hook-trust",
            "-C",
            sandbox.home.to_str().unwrap_or("."),
            &prompt,
        ],
        &sandbox,
        env,
    )
    .await;
    let mut response = if let Some(text) = extract_codex_jsonl(&raw.content) {
        ModelResponse {
            content: text,
            ..raw
        }
    } else {
        raw
    };
    if codex_coverage_hooks_enabled() {
        let coverage_path = codex_dir.join(CODEX_HOOK_COVERAGE_LOG);
        if let Ok(coverage) = fs::read_to_string(&coverage_path).await {
            if !coverage.trim().is_empty() {
                response
                    .content
                    .push_str("\n\n[brama-codex-hook-coverage]\n");
                response.content.push_str(&coverage);
            }
        }
    }
    response
}

pub async fn run_kimi(
    request: &ModelRequest,
    _agent_id: &str,
    _subscription_id: &str,
    token_json: &str,
) -> ModelResponse {
    let sandbox = match Sandbox::new() {
        Ok(s) => s,
        Err(e) => return ModelResponse::failure(&request.model, e.to_string()),
    };

    // The current Kimi Code CLI (TypeScript, @moonshot-ai/kimi-code) keeps its
    // config under ~/.kimi-code and expects a config.toml that selects the
    // official Kimi provider plus a credentials JSON with the OAuth tokens.
    let kimi_home = sandbox.home.join(".kimi-code");
    let creds_dir = kimi_home.join("credentials");
    let oauth_dir = kimi_home.join("oauth");
    if let Err(e) = fs::create_dir_all(&creds_dir).await {
        return ModelResponse::failure(&request.model, format!("mkdir creds: {e}"));
    }
    if let Err(e) = fs::create_dir_all(&oauth_dir).await {
        return ModelResponse::failure(&request.model, format!("mkdir oauth: {e}"));
    }

    let creds_path = creds_dir.join("kimi-code.json");
    if let Err(e) = fs::write(&creds_path, token_json).await {
        return ModelResponse::failure(&request.model, format!("write kimi creds: {e}"));
    }
    let _ = chmod_private(&creds_path).await;

    // The CLI's OAuth provider references this file; it can remain empty —
    // the actual tokens live in credentials/kimi-code.json.
    let oauth_path = oauth_dir.join("kimi-code");
    let _ = fs::write(&oauth_path, "").await;
    let _ = chmod_private(&oauth_path).await;

    let config_path = kimi_home.join("config.toml");
    let config_toml = r#"default_model = "kimi-code/kimi-for-coding"
default_thinking = true

[providers."managed:kimi-code"]
type = "kimi"
api_key = ""
base_url = "https://api.kimi.com/coding/v1"

[providers."managed:kimi-code".oauth]
storage = "file"
key = "oauth/kimi-code"

[models."kimi-code/kimi-for-coding"]
provider = "managed:kimi-code"
model = "kimi-for-coding"
max_context_size = 262144
capabilities = [ "thinking", "always_thinking", "image_in", "video_in", "tool_use" ]
display_name = "K2.7 Code"
"#;
    if let Err(e) = fs::write(&config_path, config_toml).await {
        return ModelResponse::failure(&request.model, format!("write kimi config: {e}"));
    }
    let _ = chmod_private(&config_path).await;

    let prompt = build_prompt_from(&request.system, &request.messages);
    let raw = run_cli(
        &request.model,
        &["kimi", "-p", &prompt, "--output-format", "stream-json"],
        &sandbox,
        HashMap::new(),
    )
    .await;

    // stream-json emits one JSON object per line. We extract assistant content
    // and drop meta / resume-hint lines.
    let response = if let Some(text) = extract_kimi_stream_json(&raw.content) {
        ModelResponse {
            content: text,
            ..raw
        }
    } else {
        raw
    };

    response
}

fn extract_codex_jsonl(stdout: &str) -> Option<String> {
    // `codex exec --json` emits JSONL events. The assistant's final answer is in
    // the last `item.completed` event whose item type is `agent_message`.
    let mut last_text: Option<String> = None;
    for line in stdout.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let value: serde_json::Value = serde_json::from_str(line).ok()?;
        if value.get("type")?.as_str()? != "item.completed" {
            continue;
        }
        let item = value.get("item")?;
        if item.get("type")?.as_str()? != "agent_message" {
            continue;
        }
        if let Some(text) = item.get("text").and_then(|t| t.as_str()) {
            last_text = Some(text.to_string());
        }
    }
    last_text
}

fn extract_kimi_stream_json(stdout: &str) -> Option<String> {
    let mut parts = Vec::new();
    for line in stdout.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let value: serde_json::Value = serde_json::from_str(line).ok()?;
        if value.get("role")?.as_str()? == "assistant" {
            if let Some(content) = value.get("content").and_then(|c| c.as_str()) {
                parts.push(content.to_string());
            }
        }
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(""))
    }
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
    run_cli(&request.model, &["opencode", "run", &prompt], &sandbox, env).await
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
