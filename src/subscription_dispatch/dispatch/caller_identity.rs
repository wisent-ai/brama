//! Who is calling: the signed agent behind one request body, or nobody.

use axum::http::HeaderMap;

use crate::crypto;
use crate::gateway::broker;

pub(crate) async fn authenticate_agent(
    headers: &HeaderMap,
    raw_body: &[u8],
) -> Result<String, String> {
    let agent_id = headers
        .get("x-agent-id")
        .and_then(|v| v.to_str().ok())
        .filter(|s| !s.is_empty())
        .map(String::from)
        .ok_or_else(|| "missing x-agent-id header".to_string())?;
    let ts = headers
        .get("x-agent-timestamp")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let sig = headers
        .get("x-agent-signature")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    let secret = broker::get_agent_auth_secret(&agent_id)
        .await
        .ok_or_else(|| "no auth secret for agent".to_string())?;

    let headers_for_check = crypto::HmacHeaders {
        agent_id: &agent_id,
        timestamp: ts,
        signature: sig,
    };
    crypto::verify_agent_hmac(&headers_for_check, raw_body, secret.expose())
        .map_err(|e| format!("auth: {e}"))?;
    Ok(agent_id)
}
