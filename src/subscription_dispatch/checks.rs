//! Native subscription status collection for CLI-backed providers.
//!
//! This is deliberately separate from normal dispatch. It materializes the
//! same donated credentials into an isolated HOME, then runs provider status
//! commands that do not start browser login flows.

use std::path::Path;
use std::time::Duration;

use serde_json::{json, Value};
use tokio::fs;
use tokio::process::Command;

use crate::gateway::broker;
use crate::subscription_dispatch::runtime::{strip_ansi, Sandbox};

const SOURCE: &str = "brama-native";


#[derive(Debug, Clone)]
pub struct CollectOptions {
    pub agent_id: String,
    pub provider: Option<String>,
    pub deep: bool,
    pub persist: bool,
}

pub async fn collect_subscription_checks(opts: CollectOptions) -> Result<Value, String> {
    let checks =
        collect_subscription_check_rows(&opts.agent_id, opts.provider.as_deref(), opts.deep)
            .await?;

    if opts.persist {
        persist_checks(&opts.agent_id, opts.provider.as_deref(), &checks);
    }

    Ok(check_result(&opts, checks))
}

pub async fn collect_subscription_check_rows(
    agent_id: &str,
    provider_filter: Option<&str>,
    deep: bool,
) -> Result<Vec<Value>, String> {
    let runtime_rows = load_active_runtime_rows(agent_id).await;
    let mut checks = Vec::new();

    for row in runtime_rows {
        let provider = string_field(&row, "provider");
        if let Some(filter) = provider_filter {
            if provider != filter {
                continue;
            }
        }
        checks.push(collect_row(agent_id, &row, deep).await);
    }

    Ok(checks)
}

fn check_result(opts: &CollectOptions, checks: Vec<Value>) -> Value {
    let mut by_status = serde_json::Map::new();
    let mut by_provider = serde_json::Map::new();
    for check in &checks {
        increment(&mut by_status, &string_field(check, "status"));
        increment(&mut by_provider, &string_field(check, "provider"));
    }

    json!({
        "ok": true,
        "agentId": opts.agent_id,
        "source": SOURCE,
        "deep": opts.deep,
        "persisted": opts.persist,
        "rows": checks.len(),
        "byStatus": by_status,
        "byProvider": by_provider,
        "checks": checks,
    })
}

async fn load_active_runtime_rows(agent_id: &str) -> Vec<Value> {
    let mut rows = Vec::new();
    for entry in broker::list_subscriptions(agent_id).await {
        if entry.status != "active" || crate::journal::is_retired(&entry.id) {
            continue;
        }
        rows.push(json!({
            "id": entry.id,
            "provider": entry.provider,
            "status": entry.status,
        }));
    }
    rows
}

async fn collect_row(agent_id: &str, row: &Value, deep: bool) -> Value {
    let provider = string_field(row, "provider");
    let subscription_id = string_field(row, "id");
    let checked_at = chrono::Utc::now().to_rfc3339();

    let sandbox = match Sandbox::new() {
        Ok(s) => s,
        Err(e) => {
            return base_check(
                agent_id,
                row,
                "failed",
                "auth_status",
                "failed",
                &checked_at,
            )
            .with_error(&format!("sandbox: {e}"));
        }
    };
    let Some(credential) = broker::subscription_credential(&subscription_id, &provider).await else {
        return base_check(agent_id, row, "failed", "auth_status", "failed", &checked_at)
            .with_error("no capability credential");
    };
    let Ok(token) = credential.expose_utf8() else {
        return base_check(agent_id, row, "failed", "auth_status", "failed", &checked_at)
            .with_error("invalid capability credential");
    };

    let result = match provider.as_str() {
        "claude_code" => check_claude(agent_id, row, token, &sandbox, &checked_at, deep).await,
        "codex" => check_codex(agent_id, row, token, &sandbox, &checked_at).await,
        "kimi" => check_kimi(agent_id, row, token, &sandbox, &checked_at, deep).await,
        "opencode" => base_check(
            agent_id,
            row,
            "configured",
            "env_status",
            "configured",
            &checked_at,
        )
        .with_metadata(json!({
            "collector": SOURCE,
            "note": "opencode runtime uses provider-specific env passed during dispatch",
        })),
        _ => base_check(
            agent_id,
            row,
            "unknown",
            "auth_status",
            "unavailable",
            &checked_at,
        )
        .with_error("no native checker for provider"),
    };

    result.with_metadata_merge(json!({
        "subscriptionId": subscription_id,
    }))
}

async fn check_claude(
    agent_id: &str,
    row: &Value,
    token: &str,
    sandbox: &Sandbox,
    checked_at: &str,
    deep: bool,
) -> Value {
    let claude_dir = sandbox.home.join(".claude");
    if let Err(e) = fs::create_dir_all(&claude_dir).await {
        return base_check(agent_id, row, "failed", "auth_status", "failed", checked_at)
            .with_error(&format!("mkdir .claude: {e}"));
    }
    let creds = materialize_claude_credentials(token);
    let creds_path = claude_dir.join(".credentials.json");
    if let Err(e) = fs::write(&creds_path, creds).await {
        return base_check(agent_id, row, "failed", "auth_status", "failed", checked_at)
            .with_error(&format!("write claude credentials: {e}"));
    }
    let _ = chmod_private(&creds_path).await;

    let out = run_status_command(
        &sandbox.home,
        "claude",
        &["auth", "status", "--json"],
        &[("CLAUDE_CODE_OAUTH_TOKEN", token)],
    )
    .await;
    let output = match out {
        Ok(o) => o,
        Err(e) => {
            return base_check(agent_id, row, "failed", "auth_status", "failed", checked_at)
                .with_error(&e);
        }
    };
    let parsed = serde_json::from_str::<Value>(&output.stdout).unwrap_or_else(|_| json!({}));
    let logged_in = parsed
        .get("loggedIn")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let base = base_check(
        agent_id,
        row,
        if logged_in { "active" } else { "inactive" },
        "auth_status",
        "observed",
        checked_at,
    )
    .with_account(string_field(&parsed, "email"))
    .with_auth_method(string_field(&parsed, "authMethod"))
    .with_plan(string_field(&parsed, "subscriptionType"))
    .with_metadata(json!({
        "collector": SOURCE,
        "cli": "claude auth status --json",
        "apiProvider": string_field(&parsed, "apiProvider"),
        "orgId": string_field(&parsed, "orgId"),
        "orgName": string_field(&parsed, "orgName"),
        "exitStatus": output.status,
    }));
    if !logged_in || !deep {
        return base;
    }

    let deep_out = run_status_command(
        &sandbox.home,
        "claude",
        &["-p", "Reply with exactly OK.", "--output-format", "json"],
        &[("CLAUDE_CODE_OAUTH_TOKEN", token)],
    )
    .await;
    match deep_out {
        Ok(deep_output) => base_check(
            agent_id,
            row,
            "active",
            "runtime_call",
            "observed",
            checked_at,
        )
        .with_account(string_field(&parsed, "email"))
        .with_auth_method(string_field(&parsed, "authMethod"))
        .with_plan(string_field(&parsed, "subscriptionType"))
        .with_metadata(json!({
            "collector": SOURCE,
            "cli": "claude -p ... --output-format json",
            "authCli": "claude auth status --json",
            "exitStatus": deep_output.status,
        })),
        Err(e) => {
            let lowered = e.to_lowercase();
            let status = if lowered.contains("hit your") && lowered.contains("limit") {
                "limited"
            } else {
                "failed"
            };
            base_check(
                agent_id,
                row,
                status,
                "runtime_call",
                "observed",
                checked_at,
            )
            .with_account(string_field(&parsed, "email"))
            .with_auth_method(string_field(&parsed, "authMethod"))
            .with_plan(string_field(&parsed, "subscriptionType"))
            .with_error(&e)
            .with_metadata(json!({
                "collector": SOURCE,
                "cli": "claude -p ... --output-format json",
                "authCli": "claude auth status --json",
            }))
        }
    }
}

async fn check_codex(
    agent_id: &str,
    row: &Value,
    token_json: &str,
    sandbox: &Sandbox,
    checked_at: &str,
) -> Value {
    let codex_dir = sandbox.home.join(".codex");
    if let Err(e) = fs::create_dir_all(&codex_dir).await {
        return base_check(agent_id, row, "failed", "auth_status", "failed", checked_at)
            .with_error(&format!("mkdir .codex: {e}"));
    }
    let auth_path = codex_dir.join("auth.json");
    if let Err(e) = fs::write(&auth_path, token_json).await {
        return base_check(agent_id, row, "failed", "auth_status", "failed", checked_at)
            .with_error(&format!("write codex auth: {e}"));
    }
    let _ = chmod_private(&auth_path).await;

    let codex_home = codex_dir.to_str().unwrap_or("");
    let out = run_status_command(
        &sandbox.home,
        "codex",
        &["login", "status"],
        &[("CODEX_HOME", codex_home)],
    )
    .await;
    let output = match out {
        Ok(o) => o,
        Err(e) => {
            return base_check(agent_id, row, "failed", "auth_status", "failed", checked_at)
                .with_error(&e);
        }
    };
    let summary = [output.stdout.trim(), output.stderr.trim()]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    let lowered = summary.to_lowercase();
    let logged_in = lowered.starts_with("logged in") || lowered.contains("logged in using");
    let auth_method = if lowered.contains("chatgpt") {
        "chatgpt"
    } else if lowered.contains("api") {
        "api"
    } else {
        ""
    };

    base_check(
        agent_id,
        row,
        if logged_in { "active" } else { "inactive" },
        "auth_status",
        "observed",
        checked_at,
    )
    .with_auth_method(auth_method.to_string())
    .with_metadata(json!({
        "collector": SOURCE,
        "cli": "codex login status",
        "summary": summary,
        "codexHome": codex_home,
        "exitStatus": output.status,
    }))
}

async fn check_kimi(
    agent_id: &str,
    row: &Value,
    token_json: &str,
    sandbox: &Sandbox,
    checked_at: &str,
    deep: bool,
) -> Value {
    if let Err(e) = materialize_kimi_home(&sandbox.home, token_json).await {
        return base_check(
            agent_id,
            row,
            "failed",
            "config_status",
            "failed",
            checked_at,
        )
        .with_error(&e);
    }

    let out = run_status_command(&sandbox.home, "kimi", &["provider", "list", "--json"], &[]).await;
    let output = match out {
        Ok(o) => o,
        Err(e) => {
            return base_check(
                agent_id,
                row,
                "failed",
                "config_status",
                "failed",
                checked_at,
            )
            .with_error(&e);
        }
    };
    let parsed = serde_json::from_str::<Value>(&output.stdout).unwrap_or_else(|_| json!({}));
    let has_provider = parsed
        .get("providers")
        .and_then(|v| v.get("managed:kimi-code"))
        .is_some();

    if !deep {
        return base_check(
            agent_id,
            row,
            if has_provider { "configured" } else { "missing" },
            "config_status",
            if has_provider { "configured" } else { "failed" },
            checked_at,
        )
        .with_auth_method("oauth".to_string())
        .with_metadata(json!({
            "collector": SOURCE,
            "cli": "kimi provider list --json",
            "note": "Kimi Code does not expose a documented billing/subscription status command; this verifies provider configuration only.",
            "exitStatus": output.status,
        }));
    }

    let deep_out = run_status_command(
        &sandbox.home,
        "kimi",
        &[
            "-p",
            "Reply with exactly OK.",
            "--output-format",
            "stream-json",
        ],
        &[],
    )
    .await;
    match deep_out {
        Ok(deep_output) => base_check(
            agent_id,
            row,
            "active",
            "runtime_call",
            "observed",
            checked_at,
        )
        .with_auth_method("oauth".to_string())
        .with_metadata(json!({
            "collector": SOURCE,
            "cli": "kimi -p ... --output-format stream-json",
            "exitStatus": deep_output.status,
        })),
        Err(e) => base_check(
            agent_id,
            row,
            "failed",
            "runtime_call",
            "failed",
            checked_at,
        )
        .with_auth_method("oauth".to_string())
        .with_error(&e),
    }
}

async fn materialize_kimi_home(home: &Path, token_json: &str) -> Result<(), String> {
    let kimi_home = home.join(".kimi-code");
    let creds_dir = kimi_home.join("credentials");
    let oauth_dir = kimi_home.join("oauth");
    fs::create_dir_all(&creds_dir)
        .await
        .map_err(|e| format!("mkdir kimi credentials: {e}"))?;
    fs::create_dir_all(&oauth_dir)
        .await
        .map_err(|e| format!("mkdir kimi oauth: {e}"))?;

    let creds_path = creds_dir.join("kimi-code.json");
    fs::write(&creds_path, token_json)
        .await
        .map_err(|e| format!("write kimi credentials: {e}"))?;
    let _ = chmod_private(&creds_path).await;

    let oauth_path = oauth_dir.join("kimi-code");
    fs::write(&oauth_path, "")
        .await
        .map_err(|e| format!("write kimi oauth marker: {e}"))?;
    let _ = chmod_private(&oauth_path).await;

    let config_path = kimi_home.join("config.toml");
    fs::write(&config_path, kimi_config_toml())
        .await
        .map_err(|e| format!("write kimi config: {e}"))?;
    let _ = chmod_private(&config_path).await;
    Ok(())
}

fn kimi_config_toml() -> &'static str {
    r#"default_model = "kimi-code/kimi-for-coding"
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
"#
}

async fn run_status_command(
    home: &Path,
    program: &str,
    args: &[&str],
    envs: &[(&str, &str)],
) -> Result<CommandOutput, String> {
    let mut cmd = Command::new(program);
    cmd.args(args)
        .env("HOME", home)
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .current_dir(home);
    for (key, value) in envs {
        cmd.env(key, value);
    }
    let output = tokio::time::timeout(Duration::from_secs(30), cmd.output())
        .await
        .map_err(|_| format!("command timed out: {program} {}", args.join(" ")))?
        .map_err(|e| format!("spawn {program}: {e}"))?;

    let stdout = strip_ansi(&String::from_utf8_lossy(&output.stdout));
    let stderr = strip_ansi(&String::from_utf8_lossy(&output.stderr));
    let status = output.status.code().unwrap_or(-1);
    if !output.status.success() {
        return Err(format!(
            "exit {status}: stderr={} stdout={}",
            truncate(&stderr, 1200),
            truncate(&stdout, 1200)
        ));
    }
    Ok(CommandOutput {
        status,
        stdout,
        stderr,
    })
}

fn persist_checks(agent_id: &str, provider: Option<&str>, rows: &[Value]) {
    let _ = provider;
    for row in rows {
        crate::journal::record_check(
            agent_id,
            &string_field(row, "provider"),
            &string_field(row, "model"),
            "",
            SOURCE,
            &string_field(row, "status"),
            None,
            &string_field(row, "checked_at"),
        );
    }
}

fn base_check(
    agent_id: &str,
    row: &Value,
    status: &str,
    check_kind: &str,
    confidence: &str,
    checked_at: &str,
) -> Value {
    let provider = string_field(row, "provider");
    json!({
        "agent_id": agent_id,
        "source": SOURCE,
        "provider": provider,
        "service": service_name(&provider),
        "subscription_id": null_if_empty(string_field(row, "id")),
        "account_identifier": Value::Null,
        "status": status,
        "auth_method": Value::Null,
        "plan": Value::Null,
        "check_kind": check_kind,
        "confidence": confidence,
        "error": Value::Null,
        "metadata": {
            "collector": SOURCE,
            "runtimeLabel": string_field(row, "key_label"),
        },
        "checked_at": checked_at,
        "updated_at": checked_at,
    })
}

trait CheckValueExt {
    fn with_error(self, error: &str) -> Value;
    fn with_account(self, account: String) -> Value;
    fn with_auth_method(self, auth_method: String) -> Value;
    fn with_plan(self, plan: String) -> Value;
    fn with_metadata(self, metadata: Value) -> Value;
    fn with_metadata_merge(self, metadata: Value) -> Value;
}

impl CheckValueExt for Value {
    fn with_error(mut self, error: &str) -> Value {
        self["error"] = json!(truncate(error, 1500));
        self
    }

    fn with_account(mut self, account: String) -> Value {
        if !account.is_empty() {
            self["account_identifier"] = json!(account);
        }
        self
    }

    fn with_auth_method(mut self, auth_method: String) -> Value {
        if !auth_method.is_empty() {
            self["auth_method"] = json!(auth_method);
        }
        self
    }

    fn with_plan(mut self, plan: String) -> Value {
        if !plan.is_empty() {
            self["plan"] = json!(plan);
        }
        self
    }

    fn with_metadata(mut self, metadata: Value) -> Value {
        self["metadata"] = metadata;
        self
    }

    fn with_metadata_merge(mut self, metadata: Value) -> Value {
        let Some(base) = self.get_mut("metadata").and_then(|v| v.as_object_mut()) else {
            self["metadata"] = metadata;
            return self;
        };
        if let Some(extra) = metadata.as_object() {
            for (key, value) in extra {
                base.insert(key.clone(), value.clone());
            }
        }
        self
    }
}

struct CommandOutput {
    status: i32,
    stdout: String,
    #[allow(dead_code)]
    stderr: String,
}

fn materialize_claude_credentials(token: &str) -> String {
    let trimmed = token.trim();
    if trimmed.starts_with('{') {
        return trimmed.to_string();
    }
    let expires_ms = chrono::Utc::now().timestamp_millis() + 24 * 60 * 60 * 1000;
    json!({
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

fn string_field(row: &Value, key: &str) -> String {
    match row.get(key) {
        Some(Value::String(value)) => value.trim().to_string(),
        Some(Value::Number(value)) => value.to_string(),
        Some(Value::Bool(value)) => value.to_string(),
        _ => String::new(),
    }
}

fn null_if_empty(value: String) -> Value {
    if value.is_empty() {
        Value::Null
    } else {
        json!(value)
    }
}

fn service_name(provider: &str) -> String {
    match provider {
        "claude_code" => "Claude Code".to_string(),
        "codex" => "Codex".to_string(),
        "kimi" => "Kimi Code".to_string(),
        "opencode" => "OpenCode".to_string(),
        _ => provider.to_string(),
    }
}

fn increment(map: &mut serde_json::Map<String, Value>, key: &str) {
    let normalized = if key.is_empty() { "unknown" } else { key };
    let current = map.get(normalized).and_then(|v| v.as_u64()).unwrap_or(0);
    map.insert(normalized.to_string(), json!(current + 1));
}

fn truncate(value: &str, max: usize) -> String {
    value.chars().take(max).collect()
}
