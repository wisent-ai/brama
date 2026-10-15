//! The candidate order `task:<name>` walks: measured quality evidence first,
//! and plan headroom only inside one score.

use serde_json::Value;
use std::collections::HashMap;

use crate::gateway::broker;
use crate::types::{GatewayRefusal, Refusal};

use super::candidates::active_supported_models_for_agent;
use super::plan_order::{route_plan_key, shuffle_within_equal, unretired};

/// A row field as text, or `None` when the row carries no text, number or
/// flag under `key`.
fn string_field(row: &Value, key: &str) -> Option<String> {
    match row.get(key) {
        Some(Value::String(value)) => Some(value.trim().to_string()),
        Some(Value::Number(value)) => Some(value.to_string()),
        Some(Value::Bool(value)) => Some(value.to_string()),
        _ => None,
    }
}

/// A check row that cannot rank: its cause, naming the task and the row.
fn malformed(task: &str, row: &Value, cause: String) -> Refusal {
    Refusal::gateway(
        GatewayRefusal::DependencyUnavailable,
        format!("task '{task}' quality check {row} cannot rank: {cause}"),
    )
}

fn score_field(row: &Value) -> Result<f64, String> {
    match row.get("score") {
        Some(score) => score
            .as_f64()
            .ok_or_else(|| format!("its score {score} is not a number")),
        None => Err("it records no score".to_string()),
    }
}

fn model_field(row: &Value) -> Result<String, String> {
    match row.get("model").and_then(|model| model.as_str()) {
        Some(model) if !model.trim().is_empty() => Ok(model.trim().to_string()),
        _ => Err("it names no model".to_string()),
    }
}

/// When the check ran. A time that is absent or not RFC 3339 refuses: read
/// as the epoch, a newer score of the same model lost to an older one.
fn checked_at_field(row: &Value) -> Result<i64, String> {
    let Some(value) = string_field(row, "checked_at") else {
        return Err("it records no checked_at time".to_string());
    };
    chrono::DateTime::parse_from_rfc3339(&value)
        .map(|dt| dt.timestamp())
        .map_err(|error| format!("its checked_at {value:?} is not an RFC 3339 time: {error}"))
}

pub(in crate::subscription_dispatch::dispatch) async fn task_quality_models(
    agent_id: &str,
    task: &str,
) -> Result<Vec<String>, Refusal> {
    let active_models = active_supported_models_for_agent(agent_id).await?;
    let rows = crate::journal::checks_for_task(agent_id, task)
        .map_err(|error| Refusal::gateway(GatewayRefusal::DependencyUnavailable, error))?;
    if rows.is_empty() {
        // A task nobody measured is a selector that names nothing here.
        return Err(Refusal::gateway(
            GatewayRefusal::InvalidRequest,
            format!("no quality checks configured for task '{task}'"),
        ));
    }
    let mut latest_by_model: HashMap<String, (f64, i64)> = HashMap::new();
    for row in rows
        .iter()
        .filter(|row| string_field(row, "status").as_deref() == Some("active"))
    {
        // An active row that names no model, no score or no time refuses:
        // passed over, the ranking stood on fewer measurements than recorded.
        let model = model_field(row).map_err(|cause| malformed(task, row, cause))?;
        if !active_models.iter().any(|active| active == &model) {
            continue;
        }
        let score = score_field(row).map_err(|cause| malformed(task, row, cause))?;
        let checked_at = checked_at_field(row).map_err(|cause| malformed(task, row, cause))?;
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
        // Measured, but on no model this agent can reach right now.
        return Err(Refusal::gateway(
            GatewayRefusal::SubscriptionUnavailable,
            format!("no active quality check result for task '{task}' and signed agent"),
        ));
    }
    // Highest score first; of two checks of equal score, the later first.
    scored.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| b.2.cmp(&a.2)));
    let subscriptions = unretired(
        broker::routing_subscriptions(agent_id)
            .map_err(|error| Refusal::gateway(GatewayRefusal::DependencyUnavailable, error))?,
    )?;
    let mut ordered = Vec::new();
    for same_score in scored.chunk_by(|left, right| (left.1 - right.1).abs() < f64::EPSILON) {
        // Quality outranks quota: inside one score the freest plan leads, and
        // only two models whose providers are equally spent are shuffled.
        let mut group: Vec<String> = same_score.iter().map(|(model, ..)| model.clone()).collect();
        group.sort_by_key(|model| route_plan_key(&subscriptions, model));
        shuffle_within_equal(&mut group, |model| route_plan_key(&subscriptions, model))?;
        ordered.extend(group);
    }
    Ok(ordered)
}
