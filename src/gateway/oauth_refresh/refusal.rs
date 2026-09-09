//! What a refused refresh means, and how it is reported.
//!
//! The asymmetry here is deliberate: a refusal is only called definitive on
//! evidence the provider itself produced, and everything else is left for the
//! next sweep. Disabling a healthy credential costs an account until someone
//! signs in again, while waiting one more minute costs a minute.

use serde_json::Value;

use crate::core::failure::{self, IMPACT_CREDENTIAL_REFRESH, POINT_OAUTH_REFRESH};
use wisent_errors::{Code, Failure};

/// One refresh failure, in the fleet's shape. The detail is whatever the layer
/// below said, word for word: a provider that answers `invalid_grant` is the
/// only thing that explains the refusal the dispatcher reports later.
pub(super) fn refresh_failure(code: Code, detail: impl Into<String>) -> Failure {
    failure::envelope(POINT_OAUTH_REFRESH, code, IMPACT_CREDENTIAL_REFRESH, detail)
}

/// The words a provider uses when a refresh token is gone for good.
///
/// Matched as text rather than by status alone because OAuth 2.0 states the
/// definitive answer in the body of an HTTP 400: a classifier that reads only
/// the status calls `invalid_grant` a mystery and keeps presenting a dead grant
/// every minute for as long as nobody reads the log.
const DEFINITIVE_REFUSALS: &[&str] = &[
    "invalid_grant",
    "invalid_token",
    "revoked",
    // The OAuth code for a client that may not use this refresh token, which is
    // the same repair as a refusal of the token itself: sign in again.
    "unauthorized_client",
];

/// What this module itself says when the *stored* document, not the provider,
/// is why a refresh cannot happen.
///
/// These are not blips and no wait changes them: the vault holds a document
/// with no refresh token in it, so every future sweep will read the same
/// document and fail the same way. Only a sign-in replaces it. Treating them
/// as transient is what kept `brama-sub-wisent-app-codex-primary` alive-looking
/// on the mini for three days -- every sweep logged `the credential is left as
/// it stands for the next sweep` about a document that could never refresh --
/// while every request for that account was refused. The sentences are
/// declared here and used by the reader below, so a reword cannot silently
/// reclassify them.
pub(super) const NO_REFRESH_TOKEN: &str = "OAuth credential has no refresh token";
pub(super) const NOT_AN_OBJECT: &str = "OAuth credential is not an object";
const STORED_DOCUMENT_REFUSALS: &[&str] = &[NO_REFRESH_TOKEN, NOT_AN_OBJECT];

/// Whether a refused refresh is the provider disowning the grant, or a blip.
pub(in crate::gateway) enum RefreshRefusal {
    /// The provider will not accept this grant again. Only a sign-in that
    /// replaces it repairs this, so the credential must stop being presented.
    Definitive,
    /// Nothing was learned about the grant. The next sweep asks again.
    Transient,
}

/// Classify one refused refresh.
pub(in crate::gateway) fn classify_refusal(failure: &Failure) -> RefreshRefusal {
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
    // Brama's own reading of the stored document. A document with no refresh
    // token cannot be refreshed by anybody, so this is a sign-in case even
    // though no provider said anything.
    if STORED_DOCUMENT_REFUSALS
        .iter()
        .any(|refusal| detail.contains(&refusal.to_ascii_lowercase()))
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

/// The provider's own words for a refused refresh.
///
/// OAuth 2.0 states the reason in `error` and `error_description`, and that
/// pair is the sentence an operator needs: `invalid_grant -- Refresh token not
/// found or invalid` says the grant is gone, which no retry repairs. A body
/// shaped some other way is carried through as it stands. Nothing here is
/// paraphrased; the provider's text is data.
fn provider_rejection_text(body: &str) -> Option<String> {
    let parsed = serde_json::from_str::<Value>(body).ok()?;
    let field = |key: &str| {
        parsed
            .get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(str::to_owned)
    };
    let code = field("error");
    let description = field("error_description")
        .or_else(|| {
            parsed
                .pointer("/error/message")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .or_else(|| field("message"));
    match (code, description) {
        (Some(code), Some(description)) => Some(format!("{code} -- {description}")),
        (Some(code), None) => Some(code),
        (None, Some(description)) => Some(description),
        (None, None) => None,
    }
}

/// What a refused refresh reports: the fleet's classification of the status the
/// provider answered with, and the provider's own sentence as the detail.
///
/// The status alone was all this used to log, and the status alone is what a day
/// went into supplementing by hand. The body says which of `invalid_grant`, a
/// revoked client or a throttle it was, so it travels with the failure.
pub(super) fn rejection_failure(status: u16, body: &str) -> Failure {
    let stated = provider_rejection_text(body).unwrap_or_else(|| body.trim().to_owned());
    let detail = if stated.is_empty() {
        format!("OAuth refresh rejected with HTTP {status}")
    } else {
        format!("OAuth refresh rejected with HTTP {status}: {stated}")
    };
    refresh_failure(Code::from_upstream_status(status), detail)
}
