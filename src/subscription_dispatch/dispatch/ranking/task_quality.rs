//! The candidate order `task:<name>` walks: measured quality evidence first,
//! and plan headroom only inside one score.

use serde_json::Value;
use std::collections::HashMap;

use crate::gateway::broker;

use super::candidates::active_supported_models_for_agent;
use super::plan_order::{ordered_float_key, route_plan_key, shuffle_within_equal};

fn string_field(row: &Value, key: &str) -> String {
    match row.get(key) {
        Some(Value::String(value)) => value.trim().to_string(),
        Some(Value::Number(value)) => value.to_string(),
        Some(Value::Bool(value)) => value.to_string(),
        _ => String::new(),
    }
}

fn score_field(row: &Value) -> Option<f64> {
    row.get("score").and_then(|score| score.as_f64())
}

fn model_field(row: &Value) -> Option<String> {
    row.get("model")
        .and_then(|model| model.as_str())
        .map(|model| model.trim().to_string())
        .filter(|model| !model.is_empty())
}

fn checked_at_field(row: &Value) -> i64 {
    let value = string_field(row, "checked_at");
    chrono::DateTime::parse_from_rfc3339(&value)
        .map(|dt| dt.timestamp())
        .unwrap_or(0)
}

pub(in crate::subscription_dispatch::dispatch) async fn task_quality_models(
    agent_id: &str,
    task: &str,
) -> Result<Vec<String>, String> {
    let active_models = active_supported_models_for_agent(agent_id).await?;
    let rows = crate::journal::checks_for_task(agent_id, task);
    if rows.is_empty() {
        return Err(format!("no quality checks configured for task '{task}'"));
    }
    let mut latest_by_model: HashMap<String, (f64, i64)> = HashMap::new();
    for row in rows
        .iter()
        .filter(|row| string_field(row, "status") == "active")
    {
        let Some(model) = model_field(row) else {
            continue;
        };
        if !active_models.iter().any(|active| active == &model) {
            continue;
        }
        let Some(score) = score_field(row) else {
            continue;
        };
        let checked_at = checked_at_field(row);
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
        return Err(format!(
            "no active quality check result for task '{task}' and signed agent"
        ));
    }
    scored.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| b.2.cmp(&a.2))
    });
    let subscriptions = broker::list_subscriptions(agent_id).await;
    let mut ordered = Vec::new();
    let mut idx = 0;
    while idx < scored.len() {
        let score = scored[idx].1;
        let mut group = Vec::new();
        while idx < scored.len() && (scored[idx].1 - score).abs() < f64::EPSILON {
            group.push(scored[idx].0.clone());
            idx += 1;
        }
        // Quality outranks quota: inside one score the freest plan leads, and
        // only two models whose providers are equally spent are shuffled.
        group.sort_by(|left, right| {
            route_plan_key(&subscriptions, left)
                .partial_cmp(&route_plan_key(&subscriptions, right))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        shuffle_within_equal(&mut group, |model| {
            ordered_float_key(route_plan_key(&subscriptions, model))
        })?;
        ordered.extend(group);
    }
    Ok(ordered)
}
