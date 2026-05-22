//! Thin wrapper around the `postgrest` crate configured with
//! `SUPABASE_URL` + `SUPABASE_SERVICE_ROLE_KEY`. Used by the gateway
//! subscription + donation handlers.

use std::env;

use postgrest::Postgrest;
use serde_json::Value;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum SupabaseError {
    #[error("SUPABASE_URL env var not set")]
    UrlMissing,
    #[error("SUPABASE_SERVICE_ROLE_KEY env var not set")]
    KeyMissing,
    #[error("postgrest http error: {0}")]
    Http(String),
    #[error("row not found")]
    NotFound,
    #[error("json parse: {0}")]
    Json(#[from] serde_json::Error),
    #[error("response body read: {0}")]
    BodyRead(String),
}

pub fn client() -> Result<Postgrest, SupabaseError> {
    let url = env::var("SUPABASE_URL")
        .map_err(|_| SupabaseError::UrlMissing)?;
    let key = env::var("SUPABASE_SERVICE_ROLE_KEY")
        .map_err(|_| SupabaseError::KeyMissing)?;
    let rest_url = format!("{}/rest/v1", url.trim_end_matches('/'));
    Ok(Postgrest::new(rest_url)
        .insert_header("apikey", &key)
        .insert_header("Authorization", format!("Bearer {key}")))
}

/// Look up the HMAC `auth_secret` for a non-trade-agent service client
/// (wisent-app, wisent-compute, growth-tactics, etc.) in the canonical
/// `model_router_clients` table. Returns None when the row is absent.
/// Schema:
///     agent_id        text primary key
///     auth_secret     text not null
///     created_at      timestamptz default now()
///     description     text
/// Separated from `trade_agent_secrets` which carries trading-autonomy
/// trade agent identities with their semantic fields (ticker, mode).
async fn get_model_router_client_secret(
    supabase: &Postgrest,
    agent_id: &str,
) -> Result<Option<String>, SupabaseError> {
    let resp = supabase
        .from("model_router_clients")
        .select("auth_secret")
        .eq("agent_id", agent_id)
        .single()
        .execute()
        .await
        .map_err(|e| SupabaseError::Http(e.to_string()))?;
    let status = resp.status();
    if status == 406 || status == 404 {
        return Ok(None);
    }
    let text = resp
        .text()
        .await
        .map_err(|e| SupabaseError::BodyRead(e.to_string()))?;
    let v: Value = serde_json::from_str(&text)?;
    Ok(v.get("auth_secret").and_then(|s| s.as_str()).map(String::from))
}

/// Legacy lookup against `trade_agent_secrets`. Kept while existing
/// wisent-app callers (and any other service still in the
/// `mode='service'` shoehorn pattern) are migrated to
/// `model_router_clients`. Drop once non-trade-agent rows are all
/// moved over.
async fn get_trade_agent_secret_legacy(
    supabase: &Postgrest,
    instance_id: &str,
) -> Result<Option<String>, SupabaseError> {
    let resp = supabase
        .from("trade_agent_secrets")
        .select("auth_secret")
        .eq("instance_id", instance_id)
        .single()
        .execute()
        .await
        .map_err(|e| SupabaseError::Http(e.to_string()))?;
    let status = resp.status();
    if status == 406 || status == 404 {
        return Ok(None);
    }
    let text = resp
        .text()
        .await
        .map_err(|e| SupabaseError::BodyRead(e.to_string()))?;
    let v: Value = serde_json::from_str(&text)?;
    Ok(v.get("auth_secret").and_then(|s| s.as_str()).map(String::from))
}

/// Look up the HMAC `auth_secret` for `agent_id`. Tries the canonical
/// `model_router_clients` table first, then the legacy
/// `trade_agent_secrets` table for backwards compatibility.
/// Returns None when neither table has a row so the caller can use
/// the master `AGENT_AUTH_SECRET` env as a last-resort path.
pub async fn get_agent_auth_secret(
    supabase: &Postgrest,
    instance_id: &str,
) -> Result<Option<String>, SupabaseError> {
    if let Some(secret) = get_model_router_client_secret(supabase, instance_id).await? {
        return Ok(Some(secret));
    }
    get_trade_agent_secret_legacy(supabase, instance_id).await
}

/// Invoke the `debit_user_balance` Postgres RPC. Returns the RPC's
/// returned JSON row ({success, new_balance, error_message}).
pub async fn debit_user_balance(
    supabase: &Postgrest,
    user_id: &str,
    amount: f64,
    reason: &str,
) -> Result<Value, SupabaseError> {
    let body = serde_json::json!({
        "p_user_id": user_id,
        "p_amount": amount,
        "p_reason": reason,
    });
    let resp = supabase
        .rpc("debit_user_balance", body.to_string())
        .execute()
        .await
        .map_err(|e| SupabaseError::Http(e.to_string()))?;
    let text = resp
        .text()
        .await
        .map_err(|e| SupabaseError::BodyRead(e.to_string()))?;
    let v: Value = serde_json::from_str(&text)?;
    // RPC returns an array with one element.
    if let Some(first) = v.as_array().and_then(|a| a.first()) {
        Ok(first.clone())
    } else {
        Ok(v)
    }
}

pub async fn log_activity(
    supabase: &Postgrest,
    instance_id: &str,
    action: &str,
    details: Value,
) {
    let row = serde_json::json!({
        "instance_id": instance_id,
        "action": action,
        "details": details,
        "revenue": 0,
        "cost": 0,
    });
    let _ = supabase
        .from("trade_activity_logs")
        .insert(row.to_string())
        .execute()
        .await;
}
