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

/// Look up the HMAC `auth_secret` for a given agent `instance_id` in
/// `trade_agent_secrets`. Returns None when the row is absent so the
/// caller can fall back to the master `AGENT_AUTH_SECRET` env.
pub async fn get_agent_auth_secret(
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
