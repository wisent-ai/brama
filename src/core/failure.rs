//! Where Brama's failures get their envelope.
//!
//! Brama had none. Every layer reported a sentence and dropped the sentence it
//! was given, so `all bounded 'codex' credentials unavailable for agent` carried
//! no code, no failure point, and no hint that the credential underneath it had
//! been told `invalid_grant` by the provider. This module is the one place that
//! turns Brama's own vocabulary into the fleet's, using `wisent_errors` for
//! everything derivable and deciding nothing itself.
//!
//! Two rules hold here. Derivation is centralised: severity, retryability and
//! outage always come from the crate, never from a call site. Rendering stays
//! local: Brama's HTTP error body, its log fields and its usage ledger keep the
//! exact shapes their readers already parse, and the envelope travels beside
//! them.
//!
//! Two audiences, two vocabularies, and the split is deliberate. What Brama
//! tells a client -- the `type` and `code` of its HTTP error body, the kind
//! prefix its provider layer writes into a message -- is unchanged and is not
//! derived from here. What Brama tells an operator is the envelope, whose code
//! is the fleet's: classified from an upstream status by the catalogue where
//! there is a status, and otherwise translated once, below, from the kind Brama
//! already decided. Where the two readings differ, both are logged; neither
//! stands in for the other.

use wisent_errors::{Code, Failure};

/// The service name every Brama envelope carries.
pub const SERVICE: &str = "brama";

/// Refusals raised while choosing a model for a request.
pub const POINT_MODEL_SELECTION: &str = "brama.dispatch.model-selection";
/// Refusals raised while choosing which credential pays for a request.
pub const POINT_CREDENTIAL_SELECTION: &str = "brama.dispatch.credential-selection";
/// Refusals raised after every bounded credential has been tried.
pub const POINT_BOUNDED_ROTATION: &str = "brama.dispatch.bounded-rotation";
/// A credential put inside a recorded block by a rate-limited answer.
pub const POINT_CREDENTIAL_BLOCK: &str = "brama.dispatch.credential-block";
/// A provider API call that answered with a failure.
pub const POINT_PROVIDER_CALL: &str = "brama.providers.provider-call";
/// An OAuth refresh the provider refused.
pub const POINT_OAUTH_REFRESH: &str = "brama.gateway.oauth-refresh";
/// A refreshed grant the vault would not store.
pub const POINT_CREDENTIAL_PERSIST: &str = "brama.gateway.credential-persist";
/// A capability that did not yield a usable credential.
pub const POINT_CREDENTIAL_REDEEM: &str = "brama.gateway.credential-redeem";
/// One routed model request, as seen at the HTTP edge.
pub const POINT_MODEL_REQUEST: &str = "brama.core.model-request";
/// The point used when a failure point itself could not be built.
const POINT_UNCLASSIFIED: &str = "brama.core.unclassified";

/// What a refused model request costs its caller.
pub const IMPACT_MODEL_REQUEST: &str = "one model request";
/// What a refused refresh costs: the credential stays as the provider left it.
pub const IMPACT_CREDENTIAL_REFRESH: &str = "one credential refresh";
/// What an unwritable refresh costs: every later request re-reads the stale grant.
pub const IMPACT_CREDENTIAL_PERSIST: &str =
    "the refreshed grant every later request would have reused";
/// What a block costs: this subscription until the block expires.
pub const IMPACT_CREDENTIAL_BLOCK: &str = "this subscription until its block expires";

/// Stated when a layer reported a failure without a reason. Nothing should reach
/// it; an envelope that reaches it is the defect the crate exists to surface.
const UNSTATED: &str = "the layer below reported no reason";

/// The fleet code for one of Brama's own kinds, or `None` when the text is not
/// one of them. Callers that hold a kind they trust use [`code_for`].
///
/// This is for the sentences Brama classified itself. A code that describes an
/// upstream's HTTP status comes from `Code::from_upstream_status`, never from
/// here: the catalogue is finer than Brama's kinds and being finer is the point.
///
/// Every arm preserves what Brama already told its clients about retrying:
/// its retryable kinds map to retryable codes and its permanent ones do not.
pub fn code_for_kind(kind: &str) -> Option<Code> {
    match kind {
        "provider_authentication" | "unauthenticated" | "credential_unauthorized" => {
            Some(Code::Auth)
        }
        "provider_rate_limited" | "subscription_unavailable" => Some(Code::RateLimit),
        "dependency_timeout" => Some(Code::Timeout),
        "dependency_unavailable" => Some(Code::InfraDown),
        // A quota that is spent is not a busy provider: no wait repairs it, and
        // the account it names is ours, which is what `config` says.
        "provider_quota_exhausted" => Some(Code::Config),
        // The four request-shaped refusals all mean the route, selector or
        // measured evidence asked for does not exist.
        "invalid_request" => Some(Code::NotFound),
        "provider_failure" | "internal_error" => Some(Code::Unknown),
        _ => None,
    }
}

/// The fleet code for a kind, unattributed when the kind is not one of Brama's.
pub fn code_for(kind: &str) -> Code {
    code_for_kind(kind).unwrap_or(Code::Unknown)
}

/// The fleet code for a failure message, read from the kind prefix Brama's
/// provider layer writes in front of it and falling back to the contract code
/// the edge already derived from the whole sentence.
pub fn code_for_message(message: &str, contract_code: &str) -> Code {
    message
        .split_once(':')
        .and_then(|(prefix, _)| code_for_kind(prefix.trim()))
        .unwrap_or_else(|| code_for(contract_code))
}

/// Build one envelope. `detail` is the reason the layer below gave, verbatim.
///
/// Infallible on purpose: a gateway that drops the report because the report
/// was malformed is back where it started. The failure points and impacts are
/// constants in this module, so the fallback arm is unreachable unless one of
/// them is edited into something invalid.
pub fn envelope(point: &str, code: Code, impact: &str, detail: impl Into<String>) -> Failure {
    let detail = detail.into();
    let detail = if detail.trim().is_empty() {
        UNSTATED
    } else {
        detail.as_str()
    };
    Failure::new(point, code, SERVICE, impact, detail)
        .or_else(|_| Failure::new(POINT_UNCLASSIFIED, code, SERVICE, IMPACT_MODEL_REQUEST, detail))
        .unwrap_or_else(|_| unreachable!("brama.core.unclassified is a valid failure point"))
}
