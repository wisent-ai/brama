//! When a stored credential's access token stops working.
//!
//! One reader answers both questions asked of it -- has this access token
//! expired, and until when is this grant good -- because a second copy of
//! this parsing is a second answer that disagrees with the first. Each
//! provider's expiry is read where that provider stores it, in the unit it
//! stores it in, as `patch_oauth_blob` writes it back: nothing is searched
//! for and no unit is guessed.

use std::time::{SystemTime, UNIX_EPOCH};

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use serde_json::Value;
use tracing::warn;
use zeroize::{Zeroize, Zeroizing};

use super::provider::{access_token, oauth_provider, zeroize_json_strings};
use crate::capability::Secret;

pub(super) const MILLIS_PER_SECOND: i64 = 1000; // https://www.bipm.org/en/measurement-units/si-prefixes milli = 10^-3

pub(super) fn millis_per_second_i64() -> i64 {
    MILLIS_PER_SECOND
}

/// This machine's clock in epoch seconds. A clock set before the epoch is
/// that many seconds negative, not the epoch itself, so an expiry written
/// from it (now plus the lifetime the provider granted) is the instant the
/// clock will actually show.
pub(super) fn now_seconds() -> i64 {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(elapsed) => elapsed.as_secs() as i64,
        Err(before) => -(before.duration().as_secs() as i64),
    }
}

/// The `exp` claim of a JWT access token, in epoch seconds as RFC 7519 states
/// it, or why the token carries none.
fn jwt_expiry(token: &str) -> Result<Option<i64>, String> {
    let mut segments = token.split('.');
    let (Some(_header), Some(payload)) = (segments.next(), segments.next()) else {
        return Err("the access token is not a JWT".to_string());
    };
    let mut decoded = Zeroizing::new(
        URL_SAFE_NO_PAD
            .decode(payload)
            .map_err(|error| format!("the access token's payload is not base64url: {error}"))?,
    );
    let parsed = serde_json::from_slice::<Value>(&decoded);
    decoded.zeroize();
    let mut claims =
        parsed.map_err(|error| format!("the access token's payload is not JSON: {error}"))?;
    // https://www.rfc-editor.org/rfc/rfc7519#section-4.1.4 "exp" is a NumericDate: seconds since the epoch
    let expiry = claims.get("exp").and_then(Value::as_i64);
    zeroize_json_strings(&mut claims);
    Ok(expiry)
}

/// The instant one provider's blob says its access token stops working, in
/// epoch seconds: Claude Code's `claudeAiOauth.expiresAt` in milliseconds,
/// Kimi's `expires_at` in seconds, and Codex's access token's own JWT `exp`.
fn provider_expiry(blob: &Value, provider: &str) -> Result<Option<i64>, String> {
    match provider {
        "claude-code" => Ok(blob
            .pointer("/claudeAiOauth/expiresAt")
            .and_then(Value::as_i64)
            .map(|millis| millis.div_euclid(MILLIS_PER_SECOND))),
        "kimi" => Ok(blob
            .get("expires_at")
            .and_then(Value::as_f64)
            .map(|seconds| seconds as i64)),
        _ => match access_token(blob, provider) {
            Some(token) => jwt_expiry(token),
            None => Ok(None),
        },
    }
}

/// The instant this credential says its access token stops working, in epoch
/// seconds, or `None` when it states none. A credential whose expiry cannot be
/// read is named in the gateway log and answered as stating none, so the
/// provider's own refusal of the token forces the refresh.
fn expiry_epoch(secret: &Secret, provider: &str) -> Option<i64> {
    oauth_provider(provider)?;
    let read = secret
        .expose_utf8()
        .map_err(|error| format!("the credential is not UTF-8: {error}"))
        .and_then(|raw| {
            serde_json::from_str::<Value>(raw)
                .map_err(|error| format!("the credential is not JSON: {error}"))
        })
        .and_then(|mut blob| {
            let expiry = provider_expiry(&blob, provider);
            zeroize_json_strings(&mut blob);
            expiry
        });
    match read {
        Ok(expiry) => expiry,
        Err(reason) => {
            warn!(
                event = "oauth_expiry_unreadable",
                provider, "{provider} credential expiry cannot be read: {reason}"
            );
            None
        }
    }
}

/// Whether this access token has expired. A token is used until the instant
/// its credential states; a request it no longer authorizes is answered by
/// the provider, and that rejection forces a refresh on the request path.
pub(in crate::gateway) fn needs_refresh(secret: &Secret, provider: &str) -> bool {
    expiry_epoch(secret, provider).is_some_and(|expiry| now_seconds() >= expiry)
}

/// The instant this credential's access token stops working, in epoch
/// milliseconds, so a reader can compare it against its own clock.
pub(in crate::gateway) fn access_token_expiry_ms(secret: &Secret, provider: &str) -> Option<i64> {
    expiry_epoch(secret, provider).map(|expiry| expiry.saturating_mul(millis_per_second_i64()))
}
