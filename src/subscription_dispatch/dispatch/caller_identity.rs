//! Who is calling: the signed agent behind one request body, or nobody.

use axum::http::HeaderMap;

use crate::crypto;
use crate::gateway::broker;

pub(crate) async fn authenticate_agent(
    headers: &HeaderMap,
    raw_body: &[u8],
) -> Result<String, String> {
    // Each header is required by name before the agent's secret is read, so
    // an absent or unreadable timestamp or signature is refused for what it
    // is, never as an HMAC mismatch.
    let header = |name: &str| {
        headers
            .get(name)
            .ok_or_else(|| format!("missing {name} header"))
            .and_then(|value| {
                value
                    .to_str()
                    .map_err(|error| format!("the {name} header is not visible ASCII: {error}"))
            })
            .and_then(|text| match text.is_empty() {
                true => Err(format!("missing {name} header")),
                false => Ok(text),
            })
    };
    let agent_id = header(crypto::hmac_auth::AGENT_ID)?.to_string();
    let ts = header(crypto::hmac_auth::AGENT_TIMESTAMP)?;
    let sig = header(crypto::hmac_auth::AGENT_SIGNATURE)?;

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
