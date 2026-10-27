//! HMAC-SHA256 verification of the `x-agent-{id,timestamp,signature}` header
//! trio, compatible with verifyAgentHmacAuth in the TypeScript trading-autonomy
//! auth helpers.
//!
//! Signed message: `{agent_id}:{timestamp}:{body_sha256_hex}`.
//! Body hash is `sha256(canonical_json_body)` hex-encoded, or empty when no
//! body is provided. No clock window is applied: a replay is refused by
//! [`super::replay::accept_once`], which accepts each signature once.

use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use thiserror::Error;

type HmacSha256 = Hmac<Sha256>;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum HmacAuthError {
    #[error("missing auth headers: {0}")]
    MissingHeader(&'static str),
    #[error("invalid timestamp: {0}")]
    InvalidTimestamp(String),
    #[error("invalid signature")]
    BadSignature,
    #[error("hmac init: {0}")]
    HmacInit(String),
    #[error("the clock cannot stamp a signed request: {0}")]
    Clock(String),
}

/// The three headers that carry the agent's identity and signature.
pub struct HmacHeaders<'a> {
    pub agent_id: &'a str,
    pub timestamp: &'a str,
    pub signature: &'a str,
}

/// Verify the HMAC trio against a known per-agent `secret`.
///
/// `body` is the raw request body bytes (not yet parsed); pass an empty slice
/// for GET requests. The TS impl hashed `JSON.stringify(body)` when present
/// and used an empty string otherwise — so callers that want byte-exact
/// compat should pass the exact bytes that Node would produce.
pub fn verify_agent_hmac(
    headers: &HmacHeaders<'_>,
    body: &[u8],
    secret: &[u8],
) -> Result<(), HmacAuthError> {
    if headers.agent_id.is_empty() {
        return Err(HmacAuthError::MissingHeader("x-agent-id"));
    }
    if headers.timestamp.is_empty() {
        return Err(HmacAuthError::MissingHeader("x-agent-timestamp"));
    }
    if headers.signature.is_empty() {
        return Err(HmacAuthError::MissingHeader("x-agent-signature"));
    }

    headers
        .timestamp
        .parse::<i64>()
        .map_err(|e: std::num::ParseIntError| HmacAuthError::InvalidTimestamp(e.to_string()))?;

    let body_hash_hex = if body.is_empty() {
        String::new()
    } else {
        let mut h = Sha256::new();
        h.update(body);
        hex::encode(h.finalize())
    };
    let message = format!(
        "{}:{}:{}",
        headers.agent_id, headers.timestamp, body_hash_hex
    );

    let mut mac =
        HmacSha256::new_from_slice(secret).map_err(|e| HmacAuthError::HmacInit(e.to_string()))?;
    mac.update(message.as_bytes());
    let expected = mac.finalize().into_bytes();
    let expected_hex = hex::encode(expected);

    // Length-check first then constant-time compare.
    let sig = headers.signature.as_bytes();
    let exp = expected_hex.as_bytes();
    if sig.len() != exp.len() {
        return Err(HmacAuthError::BadSignature);
    }
    if !bool::from(sig.ct_eq(exp)) {
        return Err(HmacAuthError::BadSignature);
    }
    Ok(())
}

/// Convenience: compute the signature a client would send for the given
/// inputs. Useful for tests and for the Rust client SDK.
pub fn compute_signature(
    agent_id: &str,
    timestamp: i64,
    body: &[u8],
    secret: &[u8],
) -> Result<String, HmacAuthError> {
    let body_hash_hex = if body.is_empty() {
        String::new()
    } else {
        let mut h = Sha256::new();
        h.update(body);
        hex::encode(h.finalize())
    };
    let message = format!("{agent_id}:{timestamp}:{body_hash_hex}");
    let mut mac =
        HmacSha256::new_from_slice(secret).map_err(|e| HmacAuthError::HmacInit(e.to_string()))?;
    mac.update(message.as_bytes());
    Ok(hex::encode(mac.finalize().into_bytes()))
}

/// The headers a client sends to sign `body` as `agent_id` with `secret`,
/// stamped now in seconds since the Unix epoch: `x-agent-id`,
/// `x-agent-timestamp` and `x-agent-signature`, the trio
/// [`verify_agent_hmac`] checks. Every Brama caller that signs a request
/// signs it here.
pub fn agent_headers(
    agent_id: &str,
    body: &[u8],
    secret: &[u8],
) -> Result<Vec<(&'static str, String)>, HmacAuthError> {
    let elapsed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|error| {
            HmacAuthError::Clock(format!("it reads before the Unix epoch: {error}"))
        })?;
    let timestamp = i64::try_from(elapsed.as_secs()).map_err(|error| {
        HmacAuthError::Clock(format!("its seconds exceed a timestamp: {error}"))
    })?;
    Ok(vec![
        ("x-agent-id", agent_id.to_string()),
        ("x-agent-timestamp", timestamp.to_string()),
        (
            "x-agent-signature",
            compute_signature(agent_id, timestamp, body, secret)?,
        ),
    ])
}
