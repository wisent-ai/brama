//! HMAC-SHA256 verification of the `x-agent-{id,timestamp,signature}` header
//! trio, compatible with verifyAgentHmacAuth in the TypeScript trading-autonomy
//! auth helpers.
//!
//! Signed message: `{agent_id}:{timestamp}:{body_sha256_hex}`.
//! Body hash is `sha256(canonical_json_body)` hex-encoded, or empty when no
//! body is provided. Timestamp window is ±300 seconds.

use std::time::{SystemTime, UNIX_EPOCH};

use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use thiserror::Error;

type HmacSha256 = Hmac<Sha256>;

const SKEW_SECS: i64 = 300;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum HmacAuthError {
    #[error("missing auth headers: {0}")]
    MissingHeader(&'static str),
    #[error("invalid timestamp: {0}")]
    InvalidTimestamp(String),
    #[error("timestamp outside ±{0}s window")]
    TimestampExpired(i64),
    #[error("invalid signature")]
    BadSignature,
    #[error("hmac init: {0}")]
    HmacInit(String),
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

    let ts: i64 = headers
        .timestamp
        .parse()
        .map_err(|e: std::num::ParseIntError| HmacAuthError::InvalidTimestamp(e.to_string()))?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    if (now - ts).abs() > SKEW_SECS {
        return Err(HmacAuthError::TimestampExpired(SKEW_SECS));
    }

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
    if sig.ct_eq(exp).unwrap_u8() != 1 {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn now_secs() -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64
    }

    #[test]
    fn round_trip_empty_body() {
        let secret = b"super-secret";
        let ts = now_secs();
        let sig = compute_signature("agent-1", ts, &[], secret).unwrap();
        verify_agent_hmac(
            &HmacHeaders {
                agent_id: "agent-1",
                timestamp: &ts.to_string(),
                signature: &sig,
            },
            &[],
            secret,
        )
        .unwrap();
    }

    #[test]
    fn round_trip_with_body() {
        let secret = b"super-secret";
        let ts = now_secs();
        let body = br#"{"x":1}"#;
        let sig = compute_signature("agent-1", ts, body, secret).unwrap();
        verify_agent_hmac(
            &HmacHeaders {
                agent_id: "agent-1",
                timestamp: &ts.to_string(),
                signature: &sig,
            },
            body,
            secret,
        )
        .unwrap();
    }

    #[test]
    fn rejects_wrong_signature() {
        let secret = b"super-secret";
        let ts = now_secs();
        let err = verify_agent_hmac(
            &HmacHeaders {
                agent_id: "agent-1",
                timestamp: &ts.to_string(),
                signature: "00".repeat(32).as_str(),
            },
            &[],
            secret,
        )
        .unwrap_err();
        assert_eq!(err, HmacAuthError::BadSignature);
    }

    #[test]
    fn rejects_expired_timestamp() {
        let secret = b"super-secret";
        let ts = now_secs() - (SKEW_SECS + 60);
        let sig = compute_signature("agent-1", ts, &[], secret).unwrap();
        let err = verify_agent_hmac(
            &HmacHeaders {
                agent_id: "agent-1",
                timestamp: &ts.to_string(),
                signature: &sig,
            },
            &[],
            secret,
        )
        .unwrap_err();
        assert!(matches!(err, HmacAuthError::TimestampExpired(_)));
    }
}
