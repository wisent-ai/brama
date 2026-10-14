//! Who is calling: the signed agent behind one request body, or nobody.

use axum::http::HeaderMap;

use crate::crypto;
use crate::gateway::broker;

pub(crate) async fn authenticate_agent(
    headers: &HeaderMap,
    raw_body: &[u8],
) -> Result<String, String> {
    // Each header is required by name: an absent timestamp or signature read
    // as empty reached the HMAC check and was refused as a mismatch, after
    // the agent's secret had already been read for nothing.
    let header = |name: &str| {
        headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| format!("missing {name} header"))
    };
    let agent_id = header("x-agent-id")?.to_string();
    let ts = header("x-agent-timestamp")?;
    let sig = header("x-agent-signature")?;

    let secret = broker::get_agent_auth_secret(&agent_id).await?;

    let headers_for_check = crypto::HmacHeaders {
        agent_id: &agent_id,
        timestamp: ts,
        signature: sig,
    };
    crypto::verify_agent_hmac(&headers_for_check, raw_body, secret.expose())
        .map_err(|e| format!("auth: {e}"))?;
    let timestamp = ts
        .parse::<i64>()
        .map_err(|e| format!("auth: invalid timestamp: {e}"))?;
    crypto::accept_once(&agent_id, timestamp, sig).map_err(|e| format!("auth: {e}"))?;
    Ok(agent_id)
}
