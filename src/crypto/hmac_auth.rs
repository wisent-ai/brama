//! Brama's signed-agent request protocol, in one place for the gateway that
//! verifies it and for Brama's own commands that call it (`brama probe`,
//! `brama subscription lease`). It uses nothing else of the gateway, so it
//! can become the package other products depend on instead of their copies.
//!
//! A signed request carries three headers: [`AGENT_ID`], [`AGENT_TIMESTAMP`]
//! (whole seconds since the Unix epoch) and [`AGENT_SIGNATURE`], the
//! lowercase hex HMAC-SHA256, under the agent's secret, of
//! `{agent_id}:{timestamp}:{body_hash}`, where `body_hash` is the lowercase
//! hex SHA-256 of the exact body bytes sent, and the empty string for a
//! request without a body. No other header takes part. The gateway accepts
//! each signature once, so a caller stamps every request anew.
//!
//! A caller signs with [`agent_headers`] and sends the three pairs it
//! returns; the gateway checks them with [`verify_agent_hmac`]. A program
//! that writes its own copy of this protocol drifts from it: a body hashed
//! when it is empty, or a fourth header the gateway never reads.

use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use thiserror::Error;

type HmacSha256 = Hmac<Sha256>;

/// The header naming the agent the request is signed as.
pub const AGENT_ID: &str = "x-agent-id";
/// The header carrying the signing moment, whole seconds since the Unix epoch.
pub const AGENT_TIMESTAMP: &str = "x-agent-timestamp";
/// The header carrying the signature.
pub const AGENT_SIGNATURE: &str = "x-agent-signature";

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

/// The body's part of the signed message: the lowercase hex SHA-256 of the
/// bytes, or the empty string for no body.
pub fn body_hash(body: &[u8]) -> String {
    if body.is_empty() {
        return String::new();
    }
    hex::encode(Sha256::digest(body))
}

/// The signature a caller sends for `body` as `agent_id` at `timestamp`.
pub fn compute_signature(
    agent_id: &str,
    timestamp: i64,
    body: &[u8],
    secret: &[u8],
) -> Result<String, HmacAuthError> {
    let message = format!("{agent_id}:{timestamp}:{}", body_hash(body));
    let mut mac =
        HmacSha256::new_from_slice(secret).map_err(|e| HmacAuthError::HmacInit(e.to_string()))?;
    mac.update(message.as_bytes());
    Ok(hex::encode(mac.finalize().into_bytes()))
}

/// Verify the three headers against the agent's `secret`. `body` is the raw
/// request body as received; an empty slice for a request without one. No
/// clock window is applied here: the gateway refuses a replay by accepting
/// each signature once.
pub fn verify_agent_hmac(
    headers: &HmacHeaders<'_>,
    body: &[u8],
    secret: &[u8],
) -> Result<(), HmacAuthError> {
    if headers.agent_id.is_empty() {
        return Err(HmacAuthError::MissingHeader(AGENT_ID));
    }
    if headers.timestamp.is_empty() {
        return Err(HmacAuthError::MissingHeader(AGENT_TIMESTAMP));
    }
    if headers.signature.is_empty() {
        return Err(HmacAuthError::MissingHeader(AGENT_SIGNATURE));
    }
    let timestamp = headers
        .timestamp
        .parse::<i64>()
        .map_err(|e| HmacAuthError::InvalidTimestamp(e.to_string()))?;
    let expected = compute_signature(headers.agent_id, timestamp, body, secret)?;
    // Length first, then a constant-time comparison of the hex text.
    let sent = headers.signature.as_bytes();
    if sent.len() != expected.len() || !bool::from(sent.ct_eq(expected.as_bytes())) {
        return Err(HmacAuthError::BadSignature);
    }
    Ok(())
}

/// The headers that sign `body` as `agent_id` with `secret`, stamped now:
/// [`AGENT_ID`], [`AGENT_TIMESTAMP`] and [`AGENT_SIGNATURE`] with their
/// values, in that order. A clock that reads before the Unix epoch, or past
/// what a timestamp holds, is refused rather than signed with a made-up time.
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
        (AGENT_ID, agent_id.to_string()),
        (AGENT_TIMESTAMP, timestamp.to_string()),
        (
            AGENT_SIGNATURE,
            compute_signature(agent_id, timestamp, body, secret)?,
        ),
    ])
}
