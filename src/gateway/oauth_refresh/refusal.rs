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

/// The OAuth error codes that disown the grant itself (RFC 6749 §5.2
/// `invalid_grant`, RFC 6750 §3.1 `invalid_token`). They are protocol values of
/// the body's `error` field, read as that field: a provider states the answer
/// there, in the body of an HTTP 400, so the status alone would call
/// `invalid_grant` a mystery and keep presenting a dead grant every sweep.
const DEFINITIVE_OAUTH_ERRORS: &[&str] = &["invalid_grant", "invalid_token"];

/// The OAuth error codes that refuse this gateway's OAuth client or its grant
/// type (RFC 6749 §5.2 `invalid_client`, `unauthorized_client`), not the
/// account's grant. A sign-in with the same client is refused the same way, so
/// they are configuration, whatever status carries them: §5.2 allows a 401 for
/// `invalid_client`, and a 401 read by status alone would block a healthy
/// account for re-authorization.
const CLIENT_OAUTH_ERRORS: &[&str] = &["invalid_client", "unauthorized_client"];

/// The failure point of a refresh that cannot happen because of the *stored*
/// document, not the provider: the vault holds a document that is not an
/// object, or one with no refresh token in it. No wait changes that, since
/// every later sweep reads the same document; only a sign-in replaces it.
/// Classified by this point, never by the sentence the failure carries.
pub(in crate::gateway) const POINT_OAUTH_STORED_DOCUMENT: &str =
    "brama.gateway.oauth-refresh.stored-document";
pub(super) const NO_REFRESH_TOKEN: &str = "OAuth credential has no refresh token";
pub(super) const NOT_AN_OBJECT: &str = "OAuth credential is not an object";

/// A refresh the stored document makes impossible, at its own failure point.
pub(super) fn stored_document_failure(detail: impl Into<String>) -> Failure {
    failure::envelope(
        POINT_OAUTH_STORED_DOCUMENT,
        Code::Config,
        IMPACT_CREDENTIAL_REFRESH,
        detail,
    )
}

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
    // Brama's own reading of the stored document. A document with no refresh
    // token cannot be refreshed by anybody, so this is a sign-in case even
    // though no provider said anything.
    if failure.failure_point == POINT_OAUTH_STORED_DOCUMENT {
        return RefreshRefusal::Definitive;
    }
    // A provider that disowned the grant was classified as Auth when its answer
    // was read (`rejection_failure`), whatever status carried it; a 401 or 403
    // that named no reason is the endpoint refusing the token it was given.
    if matches!(failure.code, Code::Auth) {
        return RefreshRefusal::Definitive;
    }
    // What is left is a transport failure that never reached the provider,
    // Brama's own configuration and shape refusals, and a body that named
    // nothing recognisable. None of them is the provider saying the grant is
    // dead, and a subscription that stores a plain API key reaches exactly
    // here, so none of them may demand a sign-in.
    RefreshRefusal::Transient
}

/// What a refusal body states, read once: its OAuth `error` code and the
/// provider's own description. A body that is not a JSON object states
/// neither, and is carried as it stands by the caller.
struct StatedRefusal {
    code: Option<String>,
    description: Option<String>,
}

impl StatedRefusal {
    /// OAuth 2.0 states the reason in `error` and `error_description`, and that
    /// pair is the sentence an operator needs: `invalid_grant -- Refresh token
    /// not found or invalid` says the grant is gone, which no retry repairs.
    /// Nothing here is paraphrased; the provider's text is data.
    fn read(body: &str) -> Self {
        let Ok(parsed) = serde_json::from_str::<Value>(body) else {
            return Self {
                code: None,
                description: None,
            };
        };
        let field = |pointer: &str| {
            parsed
                .pointer(pointer)
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|text| !text.is_empty())
                .map(str::to_owned)
        };
        Self {
            code: field("/error"),
            description: field("/error_description")
                .or_else(|| field("/error/message"))
                .or_else(|| field("/message")),
        }
    }

    /// The provider's own words, when the body stated any.
    fn text(&self) -> Option<String> {
        match (&self.code, &self.description) {
            (Some(code), Some(description)) => Some(format!("{code} -- {description}")),
            (Some(code), None) => Some(code.clone()),
            (None, Some(description)) => Some(description.clone()),
            (None, None) => None,
        }
    }
}

/// What a refused refresh reports: the fleet's classification of the status the
/// provider answered with, and the provider's own sentence as the detail.
///
/// The status alone was all this used to log, and the status alone is what a day
/// went into supplementing by hand. The body says which of `invalid_grant`, a
/// revoked client or a throttle it was, so it travels with the failure.
pub(super) fn rejection_failure(status: u16, body: &str) -> Failure {
    let stated = StatedRefusal::read(body);
    let detail = match stated.text() {
        Some(text) => format!("OAuth refresh rejected with HTTP {status}: {text}"),
        None if body.trim().is_empty() => format!("OAuth refresh rejected with HTTP {status}"),
        None => format!("OAuth refresh rejected with HTTP {status}: {}", body.trim()),
    };
    let is = |codes: &[&str]| {
        stated
            .code
            .as_deref()
            .is_some_and(|code| codes.contains(&code))
    };
    let code = if is(DEFINITIVE_OAUTH_ERRORS) {
        Code::Auth
    } else if is(CLIENT_OAUTH_ERRORS) {
        Code::Config
    } else {
        Code::from_upstream_status(status)
    };
    refresh_failure(code, detail)
}
