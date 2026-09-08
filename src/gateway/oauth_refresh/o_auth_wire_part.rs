//! Part of `oauth_refresh`, split out to keep every file inside the line limit.
#![allow(unused_imports)]

use super::*;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use serde::Serialize;
use serde_json::{json, Value};
use zeroize::{Zeroize, Zeroizing};
use crate::capability::Secret;
use crate::core::failure::{self, IMPACT_CREDENTIAL_REFRESH, POINT_OAUTH_REFRESH};
use wisent_errors::{Code, Failure};

/// One refresh failure, in the fleet's shape. The detail is whatever the layer
/// below said, word for word: a provider that answers `invalid_grant` is the
/// only thing that explains the refusal the dispatcher reports later.
pub(crate) fn refresh_failure(code: Code, detail: impl Into<String>) -> Failure {
    failure::envelope(POINT_OAUTH_REFRESH, code, IMPACT_CREDENTIAL_REFRESH, detail)
}

pub(crate) const EXPIRY_KEYS: &[&str] = &["expiresAt", "expires_at", "expires", "expiry"];

#[derive(Clone, Copy)]
pub(crate) enum OAuthWire {
    Json,
    Form,
}

pub(crate) struct OAuthProvider {
    pub(crate) token_endpoint: &'static str,
    pub(crate) client_id: &'static str,
    pub(crate) wire: OAuthWire,
}

pub(crate) fn oauth_provider(provider: &str) -> Option<OAuthProvider> {
    match provider {
        "claude-code" => Some(OAuthProvider {
            token_endpoint: "https://claude.ai/v1/oauth/token",
            client_id: "9d1c250a-e61b-44d9-88ed-5944d1962f5e",
            wire: OAuthWire::Json,
        }),
        "codex" => Some(OAuthProvider {
            token_endpoint: "https://auth.openai.com/oauth/token",
            client_id: "app_EMoamEEZ73f0CkXaXp7hrann",
            wire: OAuthWire::Form,
        }),
        "kimi" => Some(OAuthProvider {
            token_endpoint: "https://auth.kimi.com/api/oauth/token",
            client_id: "17e5f671-d194-4dfb-9706-5516cb48c098",
            wire: OAuthWire::Form,
        }),
        _ => None,
    }
}

/// Whether this provider's credentials are OAuth grants Brama can refresh at
/// all.
///
/// A caller that sweeps every subscription asks this before reading anything:
/// an API-key subscription has no access token that expires, so redeeming its
/// credential to discover that costs a vault read and learns nothing.
pub(crate) fn supports_refresh(provider: &str) -> bool {
    oauth_provider(provider).is_some()
}

#[derive(Serialize)]
pub(crate) struct OAuthRefreshRequest<'a> {
    pub(crate) grant_type: &'static str,
    pub(crate) refresh_token: &'a str,
    pub(crate) client_id: &'static str,
}

pub(crate) struct RefreshGrant {
    pub(crate) access_token: String,
    pub(crate) refresh_token: Option<String>,
    pub(crate) id_token: Option<String>,
    pub(crate) expires_in: Option<u64>,
}

impl Drop for RefreshGrant {
    fn drop(&mut self) {
        self.access_token.zeroize();
        if let Some(token) = self.refresh_token.as_mut() {
            token.zeroize();
        }
        if let Some(token) = self.id_token.as_mut() {
            token.zeroize();
        }
    }
}

pub(crate) fn expiry_margin_seconds() -> i64 {
    "60".parse().expect("valid OAuth expiry margin")
}

pub(crate) fn refresh_timeout() -> Duration {
    Duration::from_secs("15".parse().expect("valid OAuth refresh timeout"))
}

pub(crate) fn max_response_bytes() -> usize {
    "65536".parse().expect("valid OAuth response limit")
}

pub(crate) fn max_credential_bytes() -> usize {
    "8192".parse().expect("valid credential size limit")
}

pub(crate) fn epoch_millis_threshold() -> f64 {
    "100000000000"
        .parse()
        .expect("valid epoch millisecond threshold")
}

pub(crate) fn millis_per_second_f64() -> f64 {
    "1000".parse().expect("valid milliseconds per second")
}

pub(crate) fn millis_per_second_i64() -> i64 {
    "1000".parse().expect("valid milliseconds per second")
}

pub(crate) fn now_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs() as i64)
        .unwrap_or_default()
}

pub(crate) fn normalize_epoch(epoch: f64) -> i64 {
    if epoch.abs() >= epoch_millis_threshold() {
        (epoch / millis_per_second_f64()) as i64
    } else {
        epoch as i64
    }
}

pub(crate) fn expiry_epoch_seconds(value: &Value) -> Option<i64> {
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

pub(crate) fn expiry_in_value(value: &Value) -> Option<i64> {
    match value {
        Value::Object(fields) => EXPIRY_KEYS
            .iter()
            .find_map(|key| fields.get(*key).and_then(expiry_epoch_seconds))
            .or_else(|| fields.values().find_map(expiry_in_value)),
        Value::Array(values) => values.iter().find_map(expiry_in_value),
        _ => None,
    }
}

pub(crate) fn access_token<'a>(blob: &'a Value, provider: &str) -> Option<&'a str> {
    let value = match provider {
        "claude-code" => blob.get("claudeAiOauth")?.get("accessToken")?,
        "codex" => blob.get("tokens")?.get("access_token")?,
        "kimi" => blob.get("access_token")?,
        _ => return None,
    };
    value.as_str().filter(|token| !token.is_empty())
}

pub(crate) fn jwt_expiry(token: &str) -> Option<i64> {
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
///
/// One reader for three questions -- is a refresh due now, is one due inside a
/// sweep's skew window, and until when is this grant good -- because a second
/// copy of this parsing is a second answer that disagrees with the first.
pub(crate) fn expiry_epoch(secret: &Secret, provider: &str) -> Option<i64> {
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

pub(crate) fn needs_refresh(secret: &Secret, provider: &str) -> bool {
    expiry_epoch(secret, provider)
        .is_some_and(|expiry| now_seconds() + expiry_margin_seconds() >= expiry)
}

/// Whether this access token dies inside `skew`.
///
/// The refresh-ahead sweep asks a wider question than the request path does:
/// the margin above is the last moment a token can still be used, while the
/// skew is how far ahead of that moment the grant should already have been
/// replaced.
pub(crate) fn expires_within(secret: &Secret, provider: &str, skew: Duration) -> bool {
    let skew_seconds = i64::try_from(skew.as_secs()).unwrap_or(i64::MAX);
    expiry_epoch(secret, provider)
        .is_some_and(|expiry| now_seconds().saturating_add(skew_seconds) >= expiry)
}

/// The instant this credential's access token stops working, in epoch
/// milliseconds, so a reader can compare it against its own clock.
pub(crate) fn access_token_expiry_ms(secret: &Secret, provider: &str) -> Option<i64> {
    expiry_epoch(secret, provider).map(|expiry| expiry.saturating_mul(millis_per_second_i64()))
}

/// The words a provider uses when a refresh token is gone for good.
///
/// Matched as text rather than by status alone because OAuth 2.0 states the
/// definitive answer in the body of an HTTP 400: a classifier that reads only
/// the status calls `invalid_grant` a mystery and keeps presenting a dead grant
/// every minute for as long as nobody reads the log.
pub(crate) const DEFINITIVE_REFUSALS: &[&str] = &[
    "invalid_grant",
    "invalid_token",
    "revoked",
    // The OAuth code for a client that may not use this refresh token, which is
    // the same repair as a refusal of the token itself: sign in again.
    "unauthorized_client",
];

/// Whether a refused refresh is the provider disowning the grant, or a blip.
pub(crate) enum RefreshRefusal {
    /// The provider will not accept this grant again. Only a sign-in that
    /// replaces it repairs this, so the credential must stop being presented.
    Definitive,
    /// Nothing was learned about the grant. The next sweep asks again.
    Transient,
}

/// Classify one refused refresh.
///
/// The asymmetry is deliberate: a refusal is only called definitive on evidence
/// the provider itself produced, and everything else is left for the next
/// sweep. Disabling a healthy credential costs an account until someone signs
/// in again, while waiting one more minute costs a minute.
pub(crate) fn classify_refusal(failure: &Failure) -> RefreshRefusal {
    // A transport failure carries no opinion about the grant: the request never
    // reached the provider, so the provider disowned nothing. Timeouts,
    // refused connections and DNS failures all arrive here.
    if matches!(failure.code, Code::Timeout | Code::InfraDown) {
        return RefreshRefusal::Transient;
    }
    let detail = failure
        .detail
        .as_deref()
        .unwrap_or_default()
        .to_ascii_lowercase();
    if DEFINITIVE_REFUSALS
        .iter()
        .any(|refusal| detail.contains(refusal))
    {
        return RefreshRefusal::Definitive;
    }
    // A 401 or 403 that got here answered without naming a reason, and an
    // endpoint refusing the refresh token it was given is the reason. The
    // transport arm above already took the network blips that never got a
    // status at all.
    if matches!(failure.code, Code::Auth) {
        return RefreshRefusal::Definitive;
    }
    // What is left is Brama's own configuration and shape refusals, and a body
    // that named nothing recognisable. None of them is the provider saying the
    // grant is dead, and a subscription that stores a plain API key reaches
    // exactly here, so none of them may demand a sign-in.
    RefreshRefusal::Transient
}
