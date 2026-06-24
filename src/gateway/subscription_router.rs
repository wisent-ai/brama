//! Central subscription-router snapshot API.
//!
//! This endpoint intentionally lives in model-router, not in Oko. Oko and other
//! services can read one shared view instead of depending on Swift code or local
//! Oko files.

use axum::extract::Path;
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::Json;
use postgrest::Postgrest;
use serde_json::{json, Map, Value};

use crate::gateway::auth::is_caller_this_agent;
use crate::gateway::supabase;
use crate::subscription_dispatch::checks::collect_subscription_check_rows;

fn err(status: StatusCode, msg: &str) -> (StatusCode, Json<Value>) {
    (status, Json(json!({ "error": msg })))
}

pub async fn subscription_router_get(
    Path(instance_id): Path<String>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let supabase_client = match supabase::client() {
        Ok(c) => c,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    };
    let authenticated = is_caller_this_agent(&headers, &instance_id, &[], &supabase_client).await;

    let (runtime_source, mut rows) = load_runtime_pool(&supabase_client, &instance_id).await;
    let (catalog_source, mut catalog_rows) =
        load_catalog(&supabase_client, &instance_id, authenticated).await;
    let (checks_source, mut checks) =
        load_checks(&supabase_client, &instance_id, authenticated).await;
    rows.append(&mut catalog_rows);
    rows.sort_by(|a, b| row_sort_key(a).cmp(&row_sort_key(b)));
    checks.sort_by(|a, b| row_sort_key(a).cmp(&row_sort_key(b)));

    let body = json!({
        "schemaVersion": 1,
        "generatedAt": chrono::Utc::now().to_rfc3339(),
        "authenticated": authenticated,
        "scope": {
            "agentIds": [instance_id],
        },
        "summary": summary(&rows),
        "checkSummary": check_summary(&checks),
        "sources": [runtime_source, catalog_source, checks_source],
        "subscriptions": rows,
        "checks": checks,
    });
    (StatusCode::OK, Json(body))
}

async fn load_runtime_pool(client: &Postgrest, instance_id: &str) -> (Value, Vec<Value>) {
    let location = "supabase:trade_agent_subscriptions";
    let resp = client
        .from("trade_agent_subscriptions")
        .select("id,instance_id,provider,key_hint,key_label,donor_id,status,created_at,updated_at")
        .eq("instance_id", instance_id)
        .order("created_at.asc")
        .execute()
        .await;

    let response = match resp {
        Ok(r) => r,
        Err(e) => {
            return (
                source_row(
                    "model-router-runtime",
                    "supabase",
                    &format!("error: {e}"),
                    location,
                    0,
                ),
                vec![],
            );
        }
    };
    let status = response.status();
    let text = response.text().await.unwrap_or_default();
    if !status.is_success() {
        return (
            source_row(
                "model-router-runtime",
                "supabase",
                &format!("http_{}", status.as_u16()),
                location,
                0,
            ),
            vec![],
        );
    }
    let raw_rows = serde_json::from_str::<Vec<Value>>(&text).unwrap_or_default();
    let rows = raw_rows
        .iter()
        .map(|row| runtime_row(row, instance_id))
        .collect::<Vec<_>>();
    (
        source_row(
            "model-router-runtime",
            "supabase",
            "ok",
            location,
            rows.len(),
        ),
        rows,
    )
}

async fn load_catalog(
    client: &Postgrest,
    instance_id: &str,
    authenticated: bool,
) -> (Value, Vec<Value>) {
    let location = "supabase:subscription_router_entries";
    let resp = client
        .from("subscription_router_entries")
        .select("id,agent_id,source,provider,service,account_identifier,status,plan,monthly_cost_usd,period_cost_usd,expires_at,last_verified_at,metadata,created_at,updated_at")
        .order("provider.asc")
        .execute()
        .await;

    let response = match resp {
        Ok(r) => r,
        Err(e) => {
            return (
                source_row(
                    "model-router-catalog",
                    "supabase",
                    &format!("error: {e}"),
                    location,
                    0,
                ),
                vec![],
            );
        }
    };
    let status = response.status();
    let text = response.text().await.unwrap_or_default();
    if !status.is_success() {
        let source_status =
            if status.as_u16() == 404 || text.contains("subscription_router_entries") {
                "missing_table".to_string()
            } else {
                format!("http_{}", status.as_u16())
            };
        return (
            source_row(
                "model-router-catalog",
                "supabase",
                &source_status,
                location,
                0,
            ),
            vec![],
        );
    }
    let raw_rows = serde_json::from_str::<Vec<Value>>(&text).unwrap_or_default();
    let rows = raw_rows
        .iter()
        .filter(|row| {
            let agent_id = string_field(row, "agent_id");
            agent_id.is_empty() || agent_id == instance_id
        })
        .map(|row| catalog_row(row, authenticated))
        .collect::<Vec<_>>();
    (
        source_row(
            "model-router-catalog",
            "supabase",
            "ok",
            location,
            rows.len(),
        ),
        rows,
    )
}

async fn load_checks(
    client: &Postgrest,
    instance_id: &str,
    authenticated: bool,
) -> (Value, Vec<Value>) {
    let location = "supabase:subscription_router_checks";
    let resp = client
        .from("subscription_router_checks")
        .select("id,agent_id,source,provider,service,subscription_id,account_identifier,status,auth_method,plan,check_kind,confidence,error,metadata,checked_at,created_at,updated_at")
        .eq("agent_id", instance_id)
        .order("provider.asc")
        .execute()
        .await;

    let response = match resp {
        Ok(r) => r,
        Err(e) => {
            return (
                source_row(
                    "model-router-checks",
                    "supabase",
                    &format!("error: {e}"),
                    location,
                    0,
                ),
                vec![],
            );
        }
    };
    let status = response.status();
    let text = response.text().await.unwrap_or_default();
    if !status.is_success() {
        let source_status = if status.as_u16() == 404 || text.contains("subscription_router_checks")
        {
            "missing_table".to_string()
        } else {
            format!("http_{}", status.as_u16())
        };
        if source_status == "missing_table" {
            return load_live_checks(instance_id, authenticated).await;
        }
        return (
            source_row(
                "model-router-checks",
                "supabase",
                &source_status,
                location,
                0,
            ),
            vec![],
        );
    }
    let raw_rows = serde_json::from_str::<Vec<Value>>(&text).unwrap_or_default();
    if raw_rows.is_empty() {
        return load_live_checks(instance_id, authenticated).await;
    }
    let rows = raw_rows
        .iter()
        .map(|row| check_row(row, authenticated))
        .collect::<Vec<_>>();
    (
        source_row(
            "model-router-checks",
            "supabase",
            "ok",
            location,
            rows.len(),
        ),
        rows,
    )
}

async fn load_live_checks(instance_id: &str, authenticated: bool) -> (Value, Vec<Value>) {
    if !authenticated {
        return (
            source_row(
                "model-router-native-checks",
                "native_cli",
                "requires_auth",
                "live:model-router collect-subscription-checks",
                0,
            ),
            vec![],
        );
    }

    match collect_subscription_check_rows(instance_id, None, false).await {
        Ok(raw_rows) => {
            let rows = raw_rows
                .iter()
                .map(|row| check_row(row, authenticated))
                .collect::<Vec<_>>();
            (
                source_row(
                    "model-router-native-checks",
                    "native_cli",
                    "live",
                    "live:model-router collect-subscription-checks",
                    rows.len(),
                ),
                rows,
            )
        }
        Err(e) => (
            source_row(
                "model-router-native-checks",
                "native_cli",
                &format!("error: {e}"),
                "live:model-router collect-subscription-checks",
                0,
            ),
            vec![],
        ),
    }
}

fn runtime_row(row: &Value, instance_id: &str) -> Value {
    let provider = string_field(row, "provider");
    json!({
        "id": string_field(row, "id"),
        "source": "model-router",
        "sourceKind": "runtime_pool",
        "agentId": instance_id,
        "provider": provider,
        "service": service_name(&provider),
        "accountIdentifier": "",
        "status": default_string(row, "status", "unknown"),
        "plan": "",
        "periodCostUSD": Value::Null,
        "monthlyCostUSD": Value::Null,
        "expiresAt": Value::Null,
        "lastVerifiedAt": Value::Null,
        "label": string_field(row, "key_label"),
        "costStatus": "missing",
        "createdAt": clone_field(row, "created_at"),
        "updatedAt": clone_field(row, "updated_at"),
    })
}

fn catalog_row(row: &Value, authenticated: bool) -> Value {
    let period_cost = clone_field(row, "period_cost_usd");
    let monthly_cost = clone_field(row, "monthly_cost_usd");
    let has_cost = numberish(&period_cost).is_some() || numberish(&monthly_cost).is_some();
    let cost_status = if authenticated {
        if has_cost {
            "known"
        } else {
            "missing"
        }
    } else if has_cost {
        "redacted"
    } else {
        "missing"
    };
    json!({
        "id": string_field(row, "id"),
        "source": default_string(row, "source", "model-router-catalog"),
        "sourceKind": "subscription_catalog",
        "agentId": string_field(row, "agent_id"),
        "provider": string_field(row, "provider"),
        "service": string_field(row, "service"),
        "accountIdentifier": if authenticated { string_field(row, "account_identifier") } else { String::new() },
        "status": default_string(row, "status", "unknown"),
        "plan": string_field(row, "plan"),
        "periodCostUSD": if authenticated { period_cost } else { Value::Null },
        "monthlyCostUSD": if authenticated { monthly_cost } else { Value::Null },
        "expiresAt": clone_field(row, "expires_at"),
        "lastVerifiedAt": clone_field(row, "last_verified_at"),
        "label": metadata_label(row),
        "costStatus": cost_status,
        "createdAt": clone_field(row, "created_at"),
        "updatedAt": clone_field(row, "updated_at"),
    })
}

fn check_row(row: &Value, authenticated: bool) -> Value {
    json!({
        "id": string_field(row, "id"),
        "source": default_string(row, "source", "model-router-native"),
        "sourceKind": "native_check",
        "agentId": string_field(row, "agent_id"),
        "provider": string_field(row, "provider"),
        "service": string_field(row, "service"),
        "subscriptionId": if authenticated { string_field(row, "subscription_id") } else { String::new() },
        "accountIdentifier": if authenticated { string_field(row, "account_identifier") } else { String::new() },
        "status": default_string(row, "status", "unknown"),
        "authMethod": string_field(row, "auth_method"),
        "plan": string_field(row, "plan"),
        "checkKind": default_string(row, "check_kind", "auth_status"),
        "confidence": default_string(row, "confidence", "observed"),
        "error": if authenticated { string_field(row, "error") } else { String::new() },
        "label": metadata_label(row),
        "checkedAt": clone_field(row, "checked_at"),
        "createdAt": clone_field(row, "created_at"),
        "updatedAt": clone_field(row, "updated_at"),
    })
}

fn summary(rows: &[Value]) -> Value {
    let mut active = 0usize;
    let mut active_known_cost = 0usize;
    let mut active_by_provider = Map::new();
    let mut active_by_source = Map::new();
    let mut known_cost_total = 0.0;

    for row in rows {
        if !is_active(row) {
            continue;
        }
        active += 1;
        increment(
            &mut active_by_provider,
            &default_string(row, "provider", "unknown"),
        );
        increment(
            &mut active_by_source,
            &default_string(row, "source", "unknown"),
        );
        let cost = numberish(&clone_field(row, "periodCostUSD"))
            .or_else(|| numberish(&clone_field(row, "monthlyCostUSD")));
        if let Some(value) = cost {
            active_known_cost += 1;
            known_cost_total += value;
        }
    }

    json!({
        "total": rows.len(),
        "active": active,
        "inactive": rows.len().saturating_sub(active),
        "activeWithKnownCost": active_known_cost,
        "activeMissingCost": active.saturating_sub(active_known_cost),
        "knownActivePeriodCostUSD": round_money(known_cost_total),
        "activeByProvider": active_by_provider,
        "activeBySource": active_by_source,
    })
}

fn check_summary(rows: &[Value]) -> Value {
    let mut active = 0usize;
    let mut failed = 0usize;
    let mut by_provider = Map::new();
    let mut by_status = Map::new();

    for row in rows {
        let status = default_string(row, "status", "unknown").to_lowercase();
        increment(&mut by_status, &status);
        increment(
            &mut by_provider,
            &default_string(row, "provider", "unknown"),
        );
        if status == "active" {
            active += 1;
        }
        if status == "failed" || status == "revoked" || status == "expired" {
            failed += 1;
        }
    }

    json!({
        "total": rows.len(),
        "active": active,
        "failed": failed,
        "byProvider": by_provider,
        "byStatus": by_status,
    })
}

fn source_row(id: &str, kind: &str, status: &str, location: &str, count: usize) -> Value {
    json!({
        "id": id,
        "kind": kind,
        "status": status,
        "location": location,
        "rows": count,
    })
}

fn row_sort_key(row: &Value) -> String {
    [
        string_field(row, "agentId"),
        string_field(row, "service"),
        string_field(row, "provider"),
        string_field(row, "source"),
    ]
    .join("|")
}

fn is_active(row: &Value) -> bool {
    let status = string_field(row, "status").to_lowercase();
    status.is_empty() || status == "active"
}

fn increment(map: &mut Map<String, Value>, key: &str) {
    let current = map.get(key).and_then(|v| v.as_u64()).unwrap_or(0);
    map.insert(key.to_string(), json!(current + 1));
}

fn clone_field(row: &Value, key: &str) -> Value {
    row.get(key).cloned().unwrap_or(Value::Null)
}

fn string_field(row: &Value, key: &str) -> String {
    match row.get(key) {
        Some(Value::String(value)) => value.trim().to_string(),
        Some(Value::Number(value)) => value.to_string(),
        Some(Value::Bool(value)) => value.to_string(),
        _ => String::new(),
    }
}

fn default_string(row: &Value, key: &str, fallback: &str) -> String {
    let value = string_field(row, key);
    if value.is_empty() {
        fallback.to_string()
    } else {
        value
    }
}

fn numberish(value: &Value) -> Option<f64> {
    match value {
        Value::Number(number) => number.as_f64(),
        Value::String(s) => s.trim().parse::<f64>().ok(),
        _ => None,
    }
}

fn metadata_label(row: &Value) -> String {
    let Some(metadata) = row.get("metadata").and_then(|v| v.as_object()) else {
        return String::new();
    };
    for key in ["note", "source", "label"] {
        if let Some(Value::String(value)) = metadata.get(key) {
            let trimmed = value.trim();
            if !trimmed.is_empty() {
                return trimmed.to_string();
            }
        }
    }
    String::new()
}

fn service_name(provider: &str) -> String {
    match provider {
        "claude_code" => "Claude Code".to_string(),
        "codex" => "Codex".to_string(),
        "kimi" => "Kimi Code".to_string(),
        "opencode" => "OpenCode".to_string(),
        "anthropic" => "Anthropic".to_string(),
        _ => provider.to_string(),
    }
}

fn round_money(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}
