//! When a stored credential's access token stops working.
//!
//! One reader answers all three questions asked of it — is a refresh due now,
//! is one due inside a sweep's skew window, and until when is this grant good
//! — because a second copy of this parsing is a second answer that disagrees
//! with the first.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use serde_json::Value;
use zeroize::{Zeroize, Zeroizing};

use super::provider::{access_token, oauth_provider, zeroize_json_strings};
use crate::capability::Secret;

const EXPIRY_KEYS: &[&str] = &["expiresAt", "expires_at", "expires", "expiry"];

fn expiry_margin_seconds() -> i64 {
    "60".parse().expect("valid OAuth expiry margin")
}

fn epoch_millis_threshold() -> f64 {
    "100000000000"
        .parse()
        .expect("valid epoch millisecond threshold")
}

fn millis_per_second_f64() -> f64 {
    "1000".parse().expect("valid milliseconds per second")
}

pub(super) fn millis_per_second_i64() -> i64 {
    "1000".parse().expect("valid milliseconds per second")
}

pub(super) fn now_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs() as i64)
        .unwrap_or_default()
}

fn normalize_epoch(epoch: f64) -> i64 {
    if epoch.abs() >= epoch_millis_threshold() {
        (epoch / millis_per_second_f64()) as i64
    } else {
        epoch as i64
    }
}

fn expiry_epoch_seconds(value: &Value) -> Option<i64> {
    match value {
        Value::Number(number) => number.as_f64().map(normalize_epoch),
        Value::String(text) => {
            let text = text.trim();
            if let Ok(parsed) = chrono::DateTime::parse_from_rfc3339(text) {
                return Some(parsed.timestamp());
            }
            text.parse::<i64>()
                .ok()
                .map(|epoch| normalize_epoch(epoch as f64))
        }
        _ => None,
    }
}

fn expiry_in_value(value: &Value) -> Option<i64> {
    match value {
        Value::Object(fields) => EXPIRY_KEYS
            .iter()
            .find_map(|key| fields.get(*key).and_then(expiry_epoch_seconds))
            .or_else(|| fields.values().find_map(expiry_in_value)),
        Value::Array(values) => values.iter().find_map(expiry_in_value),
        _ => None,
    }
}

fn jwt_expiry(token: &str) -> Option<i64> {
    let mut segments = token.split('.');
    segments.next()?;
    let payload = segments.next()?;
    let mut decoded = Zeroizing::new(URL_SAFE_NO_PAD.decode(payload).ok()?);
    let mut claims: Value = serde_json::from_slice(&decoded).ok()?;
    decoded.zeroize();
    let expiry = claims.get("exp").and_then(expiry_epoch_seconds);
    zeroize_json_strings(&mut claims);
    expiry
}

/// The instant this credential says its access token stops working, in epoch
/// seconds, or `None` when it says nothing an expiry can be read from.
fn expiry_epoch(secret: &Secret, provider: &str) -> Option<i64> {
    oauth_provider(provider)?;
    let raw = secret.expose_utf8().ok()?;
    let mut blob: Value = match serde_json::from_str(raw) {
        Ok(Value::Object(fields)) => Value::Object(fields),
        _ => return None,
    };
    let expiry =
        expiry_in_value(&blob).or_else(|| access_token(&blob, provider).and_then(jwt_expiry));
    zeroize_json_strings(&mut blob);
    expiry
}

pub(in crate::gateway) fn needs_refresh(secret: &Secret, provider: &str) -> bool {
    expiry_epoch(secret, provider)
        .is_some_and(|expiry| now_seconds() + expiry_margin_seconds() >= expiry)
}

/// Whether this access token dies inside `skew`.
///
/// The refresh-ahead sweep asks a wider question than the request path does:
/// the margin above is the last moment a token can still be used, while the
/// skew is how far ahead of that moment the grant should already have been
/// replaced.
pub(in crate::gateway) fn expires_within(secret: &Secret, provider: &str, skew: Duration) -> bool {
    let skew_seconds = i64::try_from(skew.as_secs()).unwrap_or(i64::MAX);
    expiry_epoch(secret, provider)
        .is_some_and(|expiry| now_seconds().saturating_add(skew_seconds) >= expiry)
}

/// The instant this credential's access token stops working, in epoch
/// milliseconds, so a reader can compare it against its own clock.
pub(in crate::gateway) fn access_token_expiry_ms(secret: &Secret, provider: &str) -> Option<i64> {
    expiry_epoch(secret, provider).map(|expiry| expiry.saturating_mul(millis_per_second_i64()))
}
