use serde_json::{json, Value};

use crate::subscription_dispatch::dispatch::{
    active_supported_models_for_agent, dispatch_subscription_for_agent, provider_for,
};
use crate::types::{Message, ModelRequest};

const SOURCE: &str = "brama-task-quality";

#[derive(Debug, Clone)]
pub struct TaskQualityOptions {
    pub agent_id: String,
    pub task: String,
    pub prompt: String,
    pub expected_exact: Option<String>,
    pub expected_contains: Option<String>,
    pub persist: bool,
    pub max_models: usize,
    pub allow_provider_cost: bool,
}

/// One model's check, kept typed until it is printed: the journal and the
/// best-model choice read these fields, never a printed row read back with a
/// stand-in model or score.
struct Check {
    model: String,
    provider: Option<String>,
    passed: bool,
    score: f64,
    checked_at: String,
    row: Value,
}

fn status(passed: bool) -> &'static str {
    if passed {
        "active"
    } else {
        "failed"
    }
}

pub async fn collect_task_quality(opts: TaskQualityOptions) -> Result<Value, String> {
    if !opts.allow_provider_cost {
        return Err(
            "refusing billable task-quality collection without explicit cost acknowledgement"
                .into(),
        );
    }
    let Some(max_models) = std::num::NonZeroUsize::new(opts.max_models) else {
        return Err("max_models must be at least one".into());
    };
    if opts.task.trim().is_empty() {
        return Err("task is required".into());
    }
    if opts.prompt.trim().is_empty() {
        return Err("prompt is required".into());
    }
    if opts.expected_exact.is_none() && opts.expected_contains.is_none() {
        return Err("expected_exact or expected_contains is required".into());
    }

    let mut models = active_supported_models_for_agent(&opts.agent_id).await?;
    models.truncate(max_models.get());
    let mut checks = Vec::new();
    for model in models {
        checks.push(check_model(&opts, &model).await);
    }

    if opts.persist {
        persist_quality_rows(&opts, &checks)?;
    }

    let top_score = checks
        .iter()
        .filter(|check| check.passed)
        .map(|check| check.score)
        .reduce(f64::max);
    let best_models: Vec<String> = match top_score {
        Some(top) => checks
            .iter()
            .filter(|check| check.passed && (check.score - top).abs() < f64::EPSILON)
            .map(|check| check.model.clone())
            .collect(),
        None => Vec::new(),
    };
    // One best model only when exactly one passed at the top score.
    let best_model = match best_models.as_slice() {
        [only] => Some(only.clone()),
        _ => None,
    };
    let rows = checks.len();
    let printed: Vec<Value> = checks.into_iter().map(|check| check.row).collect();

    Ok(json!({
        "ok": true,
        "source": SOURCE,
        "agentId": opts.agent_id,
        "task": opts.task,
        "persisted": opts.persist,
        "maxModels": opts.max_models,
        "rows": rows,
        "bestModel": best_model,
        "bestModels": best_models,
        "checks": printed,
    }))
}

async fn check_model(opts: &TaskQualityOptions, model: &str) -> Check {
    let checked_at = chrono::Utc::now().to_rfc3339();
    let request = ModelRequest {
        messages: vec![Message {
            role: "user".into(),
            content: Value::String(opts.prompt.clone()),
            tool_call_id: None,
            name: None,
            tool_calls: None,
        }],
        model: model.to_string(),
        max_tokens: None,
        temperature: None,
        system: None,
        tools: None,
        tool_choice: None,
        billing_target: None,
        response_schema: None,
    };
    let resp = dispatch_subscription_for_agent(&opts.agent_id, &request).await;
    let content = resp.content.trim().to_string();
    let passed = resp.success() && expected_matches(opts, &content);
    // The verdict as the number scores are kept in: one passed, zero did not.
    let score = f64::from(u8::from(passed));
    // A model no route names a provider for is recorded with no provider, so
    // its scores never read as some provider's.
    let provider = provider_for(model).map(str::to_string);
    let row = json!({
        "agent_id": opts.agent_id,
        "source": SOURCE,
        "provider": provider,
        "service": provider.as_deref().map(service_name),
        "subscription_id": Value::Null,
        "account_identifier": opts.task,
        "status": status(passed),
        "auth_method": Value::Null,
        "plan": Value::Null,
        "check_kind": "task_quality",
        "confidence": "observed",
        // A failure carries its sentence; a success is null here.
        "error": resp.reason(),
        "metadata": {
            "task": opts.task,
            "model": model,
            "score": score,
            "prompt": opts.prompt,
            "expectedExact": opts.expected_exact,
            "expectedContains": opts.expected_contains,
            "output": content,
            "latencyMs": resp.latency_ms,
            "attempts": resp.attempts,
            "success": resp.success(),
        },
        "checked_at": checked_at,
        "updated_at": checked_at,
    });
    Check {
        model: model.to_string(),
        provider,
        passed,
        score,
        checked_at,
        row,
    }
}

fn expected_matches(opts: &TaskQualityOptions, content: &str) -> bool {
    if let Some(expected) = opts.expected_exact.as_deref() {
        return content.trim() == expected.trim();
    }
    if let Some(expected) = opts.expected_contains.as_deref() {
        return content.contains(expected);
    }
    false
}

fn persist_quality_rows(opts: &TaskQualityOptions, checks: &[Check]) -> Result<(), String> {
    for check in checks {
        // The journal attributes a check to a provider; a check of a model no
        // route names a provider for stays in the returned rows with a null
        // provider and is not journaled under an empty one.
        let Some(provider) = check.provider.as_deref() else {
            continue;
        };
        crate::journal::record_check(
            &opts.agent_id,
            provider,
            &check.model,
            &opts.task,
            SOURCE,
            status(check.passed),
            Some(check.score),
            &check.checked_at,
        )?;
    }
    Ok(())
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
