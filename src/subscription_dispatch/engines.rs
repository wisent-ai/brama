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
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use serde_json::Value;
use tokio::fs;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::time::timeout;

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

const CODEX_SUBSCRIPTION_MODEL: &str = "codex-subscription";
const CODEX_MODEL_LIST_TIMEOUT: Duration = Duration::from_secs(20);
const DOCKER_CODEX_BIN: &str = "/usr/local/lib/node_modules/@openai/codex/bin/codex.js";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexModel {
    pub id: String,
    pub input_modalities: Vec<String>,
    pub reasoning: bool,
}

fn resolve_codex_bin() -> String {
    std::env::var("BRAMA_CODEX_BIN").unwrap_or_else(|_| {
        if Path::new(DOCKER_CODEX_BIN).exists() {
            DOCKER_CODEX_BIN.to_string()
        } else {
            "codex".to_string()
        }
    })
}

async fn prepare_codex_home(sandbox: &Sandbox, token_json: &str) -> Result<PathBuf, String> {
    let codex_dir = sandbox.home.join(".codex");
    fs::create_dir_all(&codex_dir)
        .await
        .map_err(|error| format!("mkdir: {error}"))?;
    let auth_path = codex_dir.join("auth.json");
    fs::write(&auth_path, token_json)
        .await
        .map_err(|error| format!("write auth.json: {error}"))?;
    chmod_private(&auth_path)
        .await
        .map_err(|error| format!("chmod auth.json: {error}"))?;
    Ok(codex_dir)
}

fn valid_codex_model_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id.trim() == id
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_' | b':'))
}

fn parse_codex_model_list_line(line: &str) -> Result<Option<Vec<CodexModel>>, String> {
    let value: Value = serde_json::from_str(line)
        .map_err(|error| format!("invalid Codex app-server response: {error}"))?;
    if value.get("id").and_then(Value::as_u64) != Some(2) {
        return Ok(None);
    }
    if let Some(error) = value.get("error") {
        return Err(format!("Codex model/list failed: {error}"));
    }
    let rows = value
        .pointer("/result/data")
        .and_then(Value::as_array)
        .ok_or_else(|| "Codex model/list response has no result.data".to_string())?;
    let mut models = rows
        .iter()
        .filter(|row| !row.get("hidden").and_then(Value::as_bool).unwrap_or(false))
        .filter_map(|row| {
            let id = row.get("id").and_then(Value::as_str)?;
            if !valid_codex_model_id(id) {
                return None;
            }
            let input_modalities = row
                .get("inputModalities")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect();
            let reasoning = row
                .get("supportedReasoningEfforts")
                .and_then(Value::as_array)
                .is_some_and(|efforts| !efforts.is_empty());
            Some(CodexModel {
                id: id.to_owned(),
                input_modalities,
                reasoning,
            })
        })
        .collect::<Vec<_>>();
    models.sort_by(|left, right| left.id.cmp(&right.id));
    models.dedup_by(|left, right| left.id == right.id);
    if models.is_empty() {
        return Err("Codex model/list returned no visible models".into());
    }
    Ok(Some(models))
}

async fn read_codex_model_list<R>(reader: R) -> Result<Vec<CodexModel>, String>
where
    R: AsyncBufRead + Unpin,
{
    let mut lines = reader.lines();
    while let Some(line) = lines
        .next_line()
        .await
        .map_err(|error| format!("read Codex app-server response: {error}"))?
    {
        if let Some(models) = parse_codex_model_list_line(&line)? {
            return Ok(models);
        }
    }
    Err("Codex app-server exited before model/list responded".into())
}

pub async fn list_codex_models(token_json: &str) -> Result<Vec<CodexModel>, String> {
    let sandbox = Sandbox::new().map_err(|error| error.to_string())?;
    let codex_dir = prepare_codex_home(&sandbox, token_json).await?;
    let mut command = Command::new(resolve_codex_bin());
    command
        .arg("app-server")
        .arg("--stdio")
        .env("CODEX_HOME", &codex_dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let mut child = command
        .spawn()
        .map_err(|error| format!("spawn Codex app-server: {error}"))?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| "Codex app-server stdin unavailable".to_string())?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "Codex app-server stdout unavailable".to_string())?;
    let requests = concat!(
        "{\"id\":1,\"method\":\"initialize\",\"params\":{\"clientInfo\":{\"name\":\"brama\",\"version\":\"1\"},\"capabilities\":{}}}\n",
        "{\"method\":\"initialized\"}\n",
        "{\"id\":2,\"method\":\"model/list\",\"params\":{\"includeHidden\":false,\"limit\":100}}\n"
    );
    stdin
        .write_all(requests.as_bytes())
        .await
        .map_err(|error| format!("write Codex app-server request: {error}"))?;
    stdin
        .flush()
        .await
        .map_err(|error| format!("flush Codex app-server request: {error}"))?;

    let result = timeout(
        CODEX_MODEL_LIST_TIMEOUT,
        read_codex_model_list(BufReader::new(stdout)),
    )
    .await
    .map_err(|_| "Codex model/list timed out".to_string())?;
    drop(stdin);
    let _ = child.kill().await;
    let _ = child.wait().await;
    result
}

fn codex_model_override(model: &str) -> Option<&str> {
    (model != CODEX_SUBSCRIPTION_MODEL).then_some(model)
}

pub async fn run_codex(
    request: &ModelRequest,
    _agent_id: &str,
    _subscription_id: &str,
    token_json: &str,
) -> ModelResponse {
    let sandbox = match Sandbox::new() {
        Ok(sandbox) => sandbox,
        Err(error) => return ModelResponse::failure(&request.model, error.to_string()),
    };
    let codex_dir = match prepare_codex_home(&sandbox, token_json).await {
        Ok(codex_dir) => codex_dir,
        Err(error) => return ModelResponse::failure(&request.model, error),
    };
    if codex_coverage_hooks_enabled() {
        if let Err(error) = materialize_codex_coverage_hooks(&codex_dir).await {
            return ModelResponse::failure(
                &request.model,
                format!("materialize codex hooks: {error}"),
            );
        }
    }
    let prompt = build_prompt_from(&request.system, &request.messages);
    let mut env = HashMap::new();
    env.insert(
        "CODEX_HOME".to_string(),
        codex_dir.to_str().unwrap_or("").to_string(),
    );
    let mut args = vec![resolve_codex_bin(), "exec".to_string()];
    if let Some(model) = codex_model_override(&request.model) {
        args.push("--model".to_string());
        args.push(model.to_string());
    }
    args.extend([
        "--dangerously-bypass-approvals-and-sandbox".to_string(),
        "--json".to_string(),
        "--dangerously-bypass-hook-trust".to_string(),
        "-C".to_string(),
        sandbox.home.to_str().unwrap_or(".").to_string(),
        prompt,
    ]);
    let argv = args.iter().map(String::as_str).collect::<Vec<_>>();
    let raw = run_cli(&request.model, &argv, &sandbox, env).await;
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
