//! Gateway-side agent HMAC verification: reads the `x-agent-*` header trio,
//! looks up the per-agent secret (with master-env backup), and verifies the
//! signature using the shared `crypto::hmac_auth` primitives.

use std::env;

use axum::http::HeaderMap;
use postgrest::Postgrest;

use crate::crypto;
use crate::gateway::supabase;

/// Returns true iff the request was signed by the agent whose `instance_id`
/// matches the path parameter. `raw_body` is the exact bytes that the client
/// signed (empty for GETs in the current TS impl).
pub async fn is_caller_this_agent(
    headers: &HeaderMap,
    instance_id: &str,
    raw_body: &[u8],
    supabase_client: &Postgrest,
) -> bool {
    let agent_id = headers
        .get("x-agent-id")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let ts = headers
        .get("x-agent-timestamp")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let sig = headers
        .get("x-agent-signature")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if agent_id.is_empty() || agent_id != instance_id {
        return false;
    }
    let secret = match supabase::get_agent_auth_secret(supabase_client, instance_id)
        .await
    {
        Ok(Some(s)) => s,
        _ => match env::var("AGENT_AUTH_SECRET") {
            Ok(s) => s,
            Err(_) => return false,
        },
    };
    crypto::verify_agent_hmac(
        &crypto::HmacHeaders {
            agent_id,
            timestamp: ts,
            signature: sig,
        },
        raw_body,
        secret.as_bytes(),
    )
    .is_ok()
}
