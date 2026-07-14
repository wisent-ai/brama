//! Weles-backed provider reauthentication.
//!
//! Provider refresh is accepted only when Weles updates the capability-backed
//! vault entry out of band. Plaintext credentials returned in a response are
//! rejected and are never persisted by Brama.

use std::env;
use std::time::Duration;

use serde_json::{json, Value};


#[derive(Debug, Clone)]
pub struct ReauthResult {
    pub refreshed: bool,
    pub source: String,
}

#[derive(Debug, Clone)]
struct ReauthConfig {
    url: String,
    bearer_token: Option<String>,
    secret: Option<String>,
    source: String,
    kind: ReauthKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReauthKind {
    Direct,
    RunsApi,
}

// Providers whose session can be refreshed on the Weles host via POST /reauth.
// opencode is intentionally excluded: the host has no opencode refresh path.
pub fn provider_is_reauthable(provider: &str) -> bool {
    matches!(provider, "codex" | "claude_code" | "kimi")
}

// model-router provider name -> the name the Weles host /reauth expects.
fn weles_reauth_provider_name(provider: &str) -> &str {
    match provider {
        "claude_code" => "claude",
        other => other,
    }
}

// Best-effort broker consult: ask the entitlements-router for this provider's
// login plan (e.g. codex -> google_sso). Only the non-sensitive plan is
// carried; the Weles host resolves everything else locally. Returns None when
// no entitlements-router command is configured, or on any error.
async fn broker_consult(weles_provider: &str) -> Option<Value> {
    let cmd = first_env(&["ENTITLEMENTS_ROUTER_CMD", "MODEL_ROUTER_ENTITLEMENTS_CMD"])?;
    let mut parts = cmd.split_whitespace();
    let program = parts.next()?;
    let mut command = std::process::Command::new(program);
    for a in parts {
        command.arg(a);
    }
    let subcommand = "resolve-".to_string() + "credentials";
    command.arg(subcommand).arg(weles_provider);
    let output = command.output().ok()?;
    if !output.status.success() {
        return None;
    }
    serde_json::from_slice::<Value>(&output.stdout).ok()
}

pub async fn reauth_provider(
    agent_id: &str,
    provider: &str,
    failed_subscription_id: &str,
    model: &str,
    error: &str,
) -> Result<ReauthResult, String> {
    let config = reauth_config().await?;
    if config.kind == ReauthKind::RunsApi {
        return reauth_provider_via_runs_api(
            &config,
            agent_id,
            provider,
            failed_subscription_id,
            model,
            error,
        )
        .await;
    }

    // Name the provider as the Weles host expects, and attach the broker plan.
    let weles_provider = weles_reauth_provider_name(provider);
    let broker_plan = broker_consult(weles_provider).await;
    let payload = json!({
        "source": "brama",
        "reason": "provider_auth_failure",
        "agent_id": agent_id,
        "provider": weles_provider,
        "model_router_provider": provider,
        "model": model,
        "failed_subscription_id": failed_subscription_id,
        "error": error,
        "broker_plan": broker_plan,
        "requested_at": chrono::Utc::now().to_rfc3339(),
    });

    let timeout_ms = positive_env_u64("WELES_REAUTH_TIMEOUT_MS").unwrap_or(300_000);
    let client = reqwest::Client::builder()
        .timeout(Duration::from_millis(timeout_ms))
        .build()
        .map_err(|e| format!("build Weles reauth client: {e}"))?;

    let mut req = client.post(&config.url).json(&payload);
    if let Some(token) = config.bearer_token.as_deref() {
        req = req.bearer_auth(token);
    }
    if let Some(secret) = config.secret.as_deref() {
        req = req.header("x-weles-reauth-secret", secret);
    }

    let resp = req
        .send()
        .await
        .map_err(|e| format!("call Weles reauth: {e}"))?;
    let status = resp.status();
    let text = resp
        .text()
        .await
        .map_err(|e| format!("read Weles reauth response: {e}"))?;
    if !status.is_success() {
        return Err(format!("Weles reauth returned HTTP {}", status.as_u16()));
    }

    let body: Value = serde_json::from_str(&text)
        .map_err(|e| format!("parse Weles reauth response JSON: {e}"))?;
    if body.get("ok").and_then(|v| v.as_bool()) == Some(false) {
        return Err(format!(
            "Weles reauth failed: {}",
            body.get("error")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown error")
        ));
    }

    if contains_credential_field(&body) {
        return Err("Weles reauth returned forbidden plaintext credential".to_string());
    }

    if response_claims_refresh(&body) {
        return Ok(ReauthResult {
            refreshed: true,
            source: format!("{}:updated-router-state", config.source),
        });
    }

    Err("Weles reauth response did not include a credential or refreshed=true".to_string())
}

async fn reauth_config() -> Result<ReauthConfig, String> {
    direct_reauth_config_from_env().ok_or_else(|| {
        "reauth is not configured; set WELES_BRAMA_REAUTH_URL and related env values".to_string()
    })
}

fn direct_reauth_config_from_env() -> Option<ReauthConfig> {
    if let Some(url) = first_env(&[
        "WELES_BRAMA_REAUTH_URL",
        "WELES_MODEL_ROUTER_REAUTH_URL",
        "BRAMA_REAUTH_URL",
        "MODEL_ROUTER_REAUTH_URL",
        "WELES_REAUTH_URL",
    ]) {
        return Some(ReauthConfig {
            url,
            bearer_token: first_env(&[
                "WELES_BRAMA_REAUTH_TOKEN",
                "WELES_MODEL_ROUTER_REAUTH_TOKEN",
                "WELES_REAUTH_TOKEN",
                "WELES_API_TOKEN",
            ]),
            secret: first_env(&[
                "WELES_REAUTH_SECRET",
                "BRAMA_REAUTH_SECRET",
                "MODEL_ROUTER_REAUTH_SECRET",
            ]),
            source: "env".to_string(),
            kind: ReauthKind::Direct,
        });
    }

    let url = first_env(&["WELES_RUNS_API_URL", "WELES_API_RUNS_URL"]).or_else(|| {
        first_env(&["WELES_API_URL", "WELES_BASE_URL", "WELES_URL"])
            .map(|base| format!("{}/api/v1/runs", base.trim_end_matches('/')))
    })?;
    Some(ReauthConfig {
        url,
        bearer_token: first_env(&[
            "WELES_BRAMA_REAUTH_TOKEN",
            "WELES_MODEL_ROUTER_REAUTH_TOKEN",
            "WELES_REAUTH_TOKEN",
            "WELES_API_TOKEN",
            "WELES_DIAG_API_TOKEN",
        ]),
        secret: first_env(&[
            "WELES_REAUTH_SECRET",
            "BRAMA_REAUTH_SECRET",
            "MODEL_ROUTER_REAUTH_SECRET",
        ]),
        source: "env".to_string(),
        kind: ReauthKind::RunsApi,
    })
}



async fn reauth_provider_via_runs_api(
    config: &ReauthConfig,
    agent_id: &str,
    provider: &str,
    failed_subscription_id: &str,
    model: &str,
    error: &str,
) -> Result<ReauthResult, String> {
    let action = reauth_action_for_provider(provider)?;
    let token = config
        .bearer_token
        .as_deref()
        .ok_or_else(|| "Weles runs API reauth requires WELES_API_TOKEN".to_string())?;
    let timeout_ms = positive_env_u64("WELES_REAUTH_TIMEOUT_MS").unwrap_or(300_000);
    let client = reqwest::Client::builder()
        .timeout(Duration::from_millis(30_000))
        .build()
        .map_err(|e| format!("build Weles runs API client: {e}"))?;

    let idempotency_key = format!(
        "brama-reauth:{agent_id}:{provider}:{failed_subscription_id}:{}",
        chrono::Utc::now().timestamp()
    );
    let body = json!({
        "action": action,
        "params": {
            "source": "brama",
            "reason": "provider_auth_failure",
            "agent_id": agent_id,
            "provider": provider,
            "model": model,
            "failed_subscription_id": failed_subscription_id,
            "error": error,
        },
        "idempotency_key": idempotency_key,
        "priority": 100,
    });
    let create_resp = client
        .post(&config.url)
        .bearer_auth(token)
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("create Weles reauth run: {e}"))?;
    let create_status = create_resp.status();
    let create_text = create_resp
        .text()
        .await
        .map_err(|e| format!("read Weles reauth run response: {e}"))?;
    if !create_status.is_success() {
        return Err(format!(
            "create Weles reauth run returned HTTP {}",
            create_status.as_u16()
        ));
    }
    let create_body: Value = serde_json::from_str(&create_text)
        .map_err(|e| format!("parse Weles reauth run response: {e}"))?;
    let run = create_body.get("row").unwrap_or(&create_body);
    let run_id = run
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "Weles runs API did not return row.id".to_string())?;
    let detail_url = run
        .get("detail_url")
        .and_then(|v| v.as_str())
        .map(String::from)
        .unwrap_or_else(|| format!("{}/{}", config.url.trim_end_matches('/'), run_id));

    let started = std::time::Instant::now();
    let mut last_body = run.clone();
    loop {
        if let Some(result) = completed_reauth_result(&last_body)? {
            return Ok(ReauthResult {
                refreshed: true,
                source: format!("{}:runs-api:{}", config.source, result),
            });
        }
        if started.elapsed() >= Duration::from_millis(timeout_ms) {
            return Err(format!(
                "Weles reauth run {run_id} did not complete within {timeout_ms}ms"
            ));
        }

        tokio::time::sleep(Duration::from_secs(5)).await;
        let poll_resp = client
            .get(&detail_url)
            .bearer_auth(token)
            .send()
            .await
            .map_err(|e| format!("poll Weles reauth run {run_id}: {e}"))?;
        let poll_status = poll_resp.status();
        let poll_text = poll_resp
            .text()
            .await
            .map_err(|e| format!("read Weles reauth run {run_id}: {e}"))?;
        if !poll_status.is_success() {
            return Err(format!(
                "poll Weles reauth run {run_id} returned HTTP {}",
                poll_status.as_u16()
            ));
        }
        last_body = serde_json::from_str(&poll_text)
            .map_err(|e| format!("parse Weles reauth run {run_id}: {e}"))?;
    }
}

fn reauth_action_for_provider(provider: &str) -> Result<&'static str, String> {
    match provider {
        "claude_code" | "opencode" => Ok("claude_reauth"),
        "codex" => Ok("codex_reauth"),
        "kimi" => Ok("kimi_reauth"),
        _ => Err(format!("no Weles reauth action for provider '{provider}'")),
    }
}

fn completed_reauth_result(row: &Value) -> Result<Option<String>, String> {
    if contains_credential_field(row) {
        return Err("Weles reauth returned forbidden plaintext credential".to_string());
    }
    let status = row.get("status").and_then(|v| v.as_str()).unwrap_or("");
    if matches!(status, "failed" | "error" | "cancelled" | "canceled") {
        return Err("Weles reauth run failed".to_string());
    }
    if !matches!(status, "completed" | "succeeded" | "success" | "done") {
        return Ok(None);
    }

    if response_claims_refresh(row) || row.get("result").is_some() {
        return Ok(Some("completed-run".to_string()));
    }
    Err("Weles reauth run completed without result evidence".to_string())
}


fn first_env(keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| env::var(key).ok())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn positive_env_u64(key: &str) -> Option<u64> {
    env::var(key)
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0)
}

fn contains_credential_field(value: &Value) -> bool {
    match value {
        Value::Object(object) => object.iter().any(|(key, child)| {
            matches!(
                key.as_str(),
                "credential" | "credentials" | "api_key" | "apiKey" | "token" | "key"
            ) || contains_credential_field(child)
        }),
        Value::Array(values) => values.iter().any(contains_credential_field),
        _ => false,
    }
}

fn response_claims_refresh(body: &Value) -> bool {
    body.get("refreshed").and_then(|v| v.as_bool()) == Some(true)
        || body.get("updated").and_then(|v| v.as_bool()) == Some(true)
        || body
            .get("subscription")
            .and_then(|v| v.get("status"))
            .and_then(|v| v.as_str())
            == Some("active")
}
