//! One refused model request, written once: where it broke, what the caller
//! loses, the reason verbatim, and the failure underneath it.

use tracing::warn;

use crate::core::failure::{self, IMPACT_MODEL_REQUEST};
use crate::types::{ModelRequest, ModelResponse};
use wisent_errors::Failure;

/// The envelope for one refused model request: where it broke, what the caller
/// loses, the reason verbatim, and the failure underneath it when there is one.
fn refusal_envelope(
    model: &str,
    point: &str,
    kind: &str,
    message: &str,
    cause: Option<Failure>,
) -> Failure {
    let refusal = failure::envelope(
        point,
        // The kind is the caller's own answer, passed in rather than assumed:
        // an envelope that says `rate_limit` beside a `503 credential_
        // unauthorized` body sends the operator looking for a busy provider
        // while the actual break is an authorization chain. The code is looked
        // up from that kind, so the two readings cannot drift apart.
        failure::code_for(kind),
        IMPACT_MODEL_REQUEST,
        message,
    )
    .with_context("model", model);
    match cause {
        Some(cause) => refusal.caused_by(cause),
        None => refusal,
    }
}

/// Refuse one model request, saying where it broke and what the layer below
/// said, and hand the caller the sentence it has always been handed.
///
/// The message is the envelope's detail and nothing else: clients parse these
/// strings, so the envelope travels in the log beside them, never inside them.
pub(in crate::subscription_dispatch::dispatch) fn refuse(
    request: &ModelRequest,
    point: &str,
    message: String,
    cause: Option<Failure>,
) -> ModelResponse {
    refuse_as(request, point, "subscription_unavailable", message, cause)
}

/// The same refusal, naming the kind the HTTP edge will answer with.
pub(in crate::subscription_dispatch::dispatch) fn refuse_as(
    request: &ModelRequest,
    point: &str,
    kind: &str,
    message: String,
    cause: Option<Failure>,
) -> ModelResponse {
    let refusal = refusal_envelope(&request.model, point, kind, &message, cause);
    warn!(
        event = "dispatch_refused",
        model = %request.model,
        envelope = %refusal.to_json(),
        "{}",
        refusal.render()
    );
    ModelResponse::failure(&request.model, message)
}

pub(in crate::subscription_dispatch::dispatch) fn failure_detail(failure: &Failure) -> String {
    failure.detail.clone().unwrap_or_else(|| failure.render())
}

pub(in crate::subscription_dispatch::dispatch) fn remember_failure(
    slot: &mut Option<Failure>,
    failure: Failure,
) {
    let prior = slot.take();
    *slot = Some(match prior {
        Some(prior) => failure.caused_by(prior),
        None => failure,
    });
}
