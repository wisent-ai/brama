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
}

pub async fn collect_task_quality(opts: TaskQualityOptions) -> Result<Value, String> {
    if opts.task.trim().is_empty() {
        return Err("task is required".into());
    }
    if opts.prompt.trim().is_empty() {
        return Err("prompt is required".into());
    }
    if opts.expected_exact.is_none() && opts.expected_contains.is_none() {
        return Err("expected_exact or expected_contains is required".into());
    }

    let models = active_supported_models_for_agent(&opts.agent_id).await?;
    let mut rows = Vec::new();
    for model in models {
        rows.push(check_model(&opts, &model).await);
    }

    if opts.persist {
        persist_quality_rows(&opts, &rows);
    }

    let top_score = rows
        .iter()
        .filter(|row| string_field(row, "status") == "active")
        .map(score_field)
        .fold(None, |best: Option<f64>, score| {
            Some(best.map_or(score, |existing| existing.max(score)))
        });
    let best_models = top_score
        .map(|score| {
            rows.iter()
                .filter(|row| string_field(row, "status") == "active")
                .filter(|row| (score_field(row) - score).abs() < f64::EPSILON)
                .filter_map(|row| {
                    row.get("metadata")
                        .and_then(|metadata| metadata.get("model"))
                        .and_then(|model| model.as_str())
                        .map(String::from)
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let best_model = if best_models.len() == 1 {
        best_models.first().cloned()
    } else {
        None
    };

    Ok(json!({
        "ok": true,
        "source": SOURCE,
        "agentId": opts.agent_id,
        "task": opts.task,
        "persisted": opts.persist,
        "rows": rows.len(),
        "bestModel": best_model,
        "bestModels": best_models,
        "checks": rows,
    }))
}

async fn check_model(opts: &TaskQualityOptions, model: &str) -> Value {
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
        max_tokens: 96,
        temperature: 0.0,
        system: None,
        tools: None,
        billing_target: None,
        subscription_decision_id: None,
    };
    let resp = dispatch_subscription_for_agent(&opts.agent_id, &request).await;
    let content = resp.content.trim().to_string();
    let passed = resp.success && expected_matches(opts, &content);
    let score = if passed { 1.0 } else { 0.0 };
    let provider = provider_for(model).unwrap_or("unknown");
    json!({
        "agent_id": opts.agent_id,
        "source": SOURCE,
        "provider": provider,
        "service": service_name(provider),
        "subscription_id": Value::Null,
        "account_identifier": opts.task,
        "status": if passed { "active" } else { "failed" },
        "auth_method": Value::Null,
        "plan": Value::Null,
        "check_kind": "task_quality",
        "confidence": "observed",
        "error": if resp.success { Value::Null } else { json!(resp.error.unwrap_or_default()) },
        "metadata": {
            "task": opts.task,
            "model": model,
            "score": score,
            "prompt": opts.prompt,
            "expectedExact": opts.expected_exact,
            "expectedContains": opts.expected_contains,
            "output": truncate(&content, 1500),
            "latencyMs": resp.latency_ms,
            "success": resp.success,
        },
        "checked_at": checked_at,
        "updated_at": checked_at,
    })
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

fn persist_quality_rows(opts: &TaskQualityOptions, rows: &[Value]) {
    for row in rows {
        let model = row
            .get("metadata")
            .and_then(|m| m.get("model"))
            .and_then(Value::as_str)
            .unwrap_or("");
        let score = row
            .get("metadata")
            .and_then(|m| m.get("score"))
            .and_then(Value::as_f64);
        crate::journal::record_check(
            &opts.agent_id,
            &string_field(row, "provider"),
            model,
            &opts.task,
            SOURCE,
            &string_field(row, "status"),
            score,
            &string_field(row, "checked_at"),
        );
    }
}

fn score_field(row: &Value) -> f64 {
    row.get("metadata")
        .and_then(|metadata| metadata.get("score"))
        .and_then(|score| score.as_f64())
        .unwrap_or(0.0)
}

fn string_field(row: &Value, key: &str) -> String {
    match row.get(key) {
        Some(Value::String(value)) => value.trim().to_string(),
        Some(Value::Number(value)) => value.to_string(),
        Some(Value::Bool(value)) => value.to_string(),
        _ => String::new(),
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

fn truncate(value: &str, max: usize) -> String {
    value.chars().take(max).collect()
}
