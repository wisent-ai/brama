//! Donation + CRUD for `trade_agent_subscriptions`. Mirrors
//! trading-autonomy/web/app/api/agents/[id]/subscriptions/route.ts.
//!
//! Mounted by `core::server` at:
//!   GET    /v1/subscriptions/{instance_id}
//!   POST   /v1/subscriptions/{instance_id}
//!   DELETE /v1/subscriptions/{instance_id}

use axum::extract::Path;
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::crypto;
use crate::gateway::auth::is_caller_this_agent;
use crate::gateway::supabase;

const VALID_PROVIDERS: &[&str] =
    &["claude_code", "openai", "codex", "kimi", "anthropic"];

#[derive(Debug, Deserialize)]
pub struct SubscribeBody {
    pub user_id: String,
    pub provider: String,
    pub api_key: String,
    #[serde(default)]
    pub label: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct RevokeBody {
    pub user_id: String,
    pub subscription_id: String,
}

fn err(status: StatusCode, msg: &str) -> (StatusCode, Json<Value>) {
    (status, Json(json!({ "error": msg })))
}

pub async fn subscriptions_get(
    Path(instance_id): Path<String>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let supabase_client = match supabase::client() {
        Ok(c) => c,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    };

    let resp = supabase_client
        .from("trade_agent_subscriptions")
        .select("id,provider,key_hint,key_label,key_encrypted,donor_id,status,created_at")
        .eq("instance_id", &instance_id)
        .eq("status", "active")
        .execute()
        .await;
    let rows = match resp {
        Ok(r) => {
            let t = r.text().await.unwrap_or_default();
            serde_json::from_str::<Vec<Value>>(&t).unwrap_or_default()
        }
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    };

    let authed =
        is_caller_this_agent(&headers, &instance_id, &[], &supabase_client).await;

    if authed {
        let mut keys = serde_json::Map::new();
        for r in &rows {
            let provider = r.get("provider").and_then(|v| v.as_str()).unwrap_or("");
            let encrypted =
                r.get("key_encrypted").and_then(|v| v.as_str()).unwrap_or("");
            if let Ok(pt) = crypto::decrypt(encrypted) {
                keys.insert(provider.to_string(), Value::String(pt));
            }
        }
        return (StatusCode::OK, Json(json!({ "keys": keys })));
    }

    // Public metadata only.
    let subs: Vec<Value> = rows
        .into_iter()
        .map(|r| {
            json!({
                "id": r.get("id"),
                "provider": r.get("provider"),
                "key_hint": r.get("key_hint"),
                "key_label": r.get("key_label"),
                "donor_id": r.get("donor_id"),
                "status": r.get("status"),
                "created_at": r.get("created_at"),
            })
        })
        .collect();
    (StatusCode::OK, Json(json!({ "subscriptions": subs })))
}

pub async fn subscriptions_post(
    Path(instance_id): Path<String>,
    Json(body): Json<SubscribeBody>,
) -> impl IntoResponse {
    if body.user_id.is_empty() {
        return err(StatusCode::BAD_REQUEST, "user_id required");
    }
    if !VALID_PROVIDERS.contains(&body.provider.as_str()) {
        return err(
            StatusCode::BAD_REQUEST,
            &format!("Invalid provider. Must be one of: {}", VALID_PROVIDERS.join(", ")),
        );
    }
    if body.api_key.len() < 8 {
        return err(
            StatusCode::BAD_REQUEST,
            "api_key required (min 8 characters)",
        );
    }

    let supabase_client = match supabase::client() {
        Ok(c) => c,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    };

    let agent_resp = supabase_client
        .from("trade_agents")
        .select("id,name")
        .eq("instance_id", &instance_id)
        .single()
        .execute()
        .await;
    let agent_row: Value = match agent_resp {
        Ok(r) if r.status().is_success() => {
            let t = r.text().await.unwrap_or_default();
            serde_json::from_str(&t).unwrap_or(json!({}))
        }
        _ => return err(StatusCode::NOT_FOUND, "Agent not found"),
    };

    let encrypted = match crypto::encrypt(&body.api_key) {
        Ok(e) => e,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    };
    let key_hint: String = body.api_key.chars().rev().take(4).collect::<String>()
        .chars().rev().collect();

    let insert = json!({
        "agent_id": agent_row.get("id"),
        "instance_id": instance_id,
        "provider": body.provider,
        "key_encrypted": encrypted,
        "key_hint": key_hint,
        "key_label": body.label,
        "donor_id": body.user_id,
        "status": "active",
    });
    let ins_resp = supabase_client
        .from("trade_agent_subscriptions")
        .insert(insert.to_string())
        .execute()
        .await;
    let sub: Value = match ins_resp {
        Ok(r) if r.status().is_success() => {
            let t = r.text().await.unwrap_or_default();
            serde_json::from_str::<Vec<Value>>(&t)
                .unwrap_or_default()
                .into_iter()
                .next()
                .unwrap_or(json!({}))
        }
        Ok(r) => {
            let t = r.text().await.unwrap_or_default();
            return err(StatusCode::INTERNAL_SERVER_ERROR, &t);
        }
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    };

    supabase::log_activity(
        &supabase_client,
        &instance_id,
        "subscription_donated",
        json!({
            "provider": body.provider,
            "key_hint": key_hint,
            "label": body.label,
            "donor_id": body.user_id,
        }),
    )
    .await;

    (
        StatusCode::OK,
        Json(json!({ "success": true, "subscription": sub })),
    )
}

pub async fn subscriptions_delete(
    Path(instance_id): Path<String>,
    Json(body): Json<RevokeBody>,
) -> impl IntoResponse {
    if body.user_id.is_empty() || body.subscription_id.is_empty() {
        return err(
            StatusCode::BAD_REQUEST,
            "user_id and subscription_id required",
        );
    }
    let supabase_client = match supabase::client() {
        Ok(c) => c,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    };

    let sub_resp = supabase_client
        .from("trade_agent_subscriptions")
        .select("id,donor_id,instance_id")
        .eq("id", &body.subscription_id)
        .single()
        .execute()
        .await;
    let sub: Value = match sub_resp {
        Ok(r) if r.status().is_success() => {
            let t = r.text().await.unwrap_or_default();
            serde_json::from_str(&t).unwrap_or(json!({}))
        }
        _ => return err(StatusCode::NOT_FOUND, "Subscription not found"),
    };
    if sub.get("instance_id").and_then(|v: &Value| v.as_str()) != Some(instance_id.as_str()) {
        return err(StatusCode::FORBIDDEN, "Subscription does not belong to this agent");
    }
    if sub.get("donor_id").and_then(|v: &Value| v.as_str()) != Some(body.user_id.as_str()) {
        return err(
            StatusCode::FORBIDDEN,
            "Only the original donor can revoke a subscription",
        );
    }

    let update = json!({
        "status": "revoked",
        "updated_at": chrono::Utc::now().to_rfc3339(),
    });
    let upd_resp = supabase_client
        .from("trade_agent_subscriptions")
        .eq("id", &body.subscription_id)
        .update(update.to_string())
        .execute()
        .await;
    if let Err(e) = upd_resp {
        return err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string());
    }

    supabase::log_activity(
        &supabase_client,
        &instance_id,
        "subscription_revoked",
        json!({
            "subscription_id": body.subscription_id,
            "donor_id": body.user_id,
        }),
    )
    .await;

    (StatusCode::OK, Json(json!({ "success": true })))
}
