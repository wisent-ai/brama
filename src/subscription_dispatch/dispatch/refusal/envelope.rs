//! One refused model request, written once: where it broke, what the caller
//! loses, the reason verbatim, and the failure underneath it.

use tracing::warn;

use crate::core::failure::{self, IMPACT_MODEL_REQUEST};
use crate::types::{GatewayRefusal, ModelRequest, ModelResponse, ProviderRefusal};
use wisent_errors::Failure;

/// The envelope for one refused model request: where it broke, what the caller
/// loses, the reason verbatim, and the failure underneath it when there is one.
fn refusal_envelope(
    model: &str,
    point: &str,
    class: ProviderRefusal,
    message: &str,
    cause: Option<Failure>,
) -> Failure {
    let refusal = failure::envelope(
        point,
        // The code is looked up from the class the caller is answered with,
        // so the log and the HTTP body cannot drift apart: an envelope that
        // says `rate_limit` beside a `503 credential_unauthorized` body sends
        // the operator looking for a busy provider while the actual break is
        // an authorization chain.
        failure::code_for(class.contract_kind()),
        IMPACT_MODEL_REQUEST,
        message,
    )
    .with_context("model", model);
    match cause {
        Some(cause) => refusal.caused_by(cause),
        None => refusal,
    }
}

/// Refuse one model request on Brama's own account, saying where it broke,
/// what class of refusal it is, and what the layer below said.
///
/// The message is the envelope's detail and nothing else: clients parse these
/// strings, so the envelope travels in the log beside them, never inside them.
pub(in crate::subscription_dispatch::dispatch) fn refuse(
    request: &ModelRequest,
    point: &str,
    class: GatewayRefusal,
    message: String,
    cause: Option<Failure>,
) -> ModelResponse {
    refuse_as(
        request,
        point,
        ProviderRefusal::Gateway(class),
        message,
        cause,
    )
}

/// The same refusal for any class, including a provider's own class carried
/// up from the call that refused.
pub(in crate::subscription_dispatch::dispatch) fn refuse_as(
    request: &ModelRequest,
    point: &str,
    class: ProviderRefusal,
    message: String,
    cause: Option<Failure>,
) -> ModelResponse {
    let refusal = refusal_envelope(&request.model, point, class, &message, cause);
    warn!(
        event = "dispatch_refused",
        model = %request.model,
        envelope = %refusal.to_json(),
        "{}",
        refusal.render()
    );
    ModelResponse::failure(&request.model, class, message)
}

/// The class a credential the vault would not produce is answered with: a
/// vault or router that did not answer (an outage a wait can repair, as the
/// fleet's code states it) is a dependency; every other refusal is a broken
/// authorization chain only an operator repairs.
pub(in crate::subscription_dispatch::dispatch) fn credential_refusal_class(
    failure: &Failure,
) -> GatewayRefusal {
    if failure.outage() && failure.retryable() {
        GatewayRefusal::DependencyUnavailable
    } else {
        GatewayRefusal::CredentialUnauthorized
    }
}

/// The reason a failure carries, or its whole rendering when it states none.
pub(in crate::subscription_dispatch::dispatch) fn failure_detail(failure: &Failure) -> String {
    match &failure.detail {
        Some(detail) => detail.clone(),
        None => failure.render(),
    }
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
