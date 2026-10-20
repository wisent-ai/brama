//! What happens to one finished call besides being answered: the process
//! counters, the one log line an operator reads it from, and the refusal
//! document a caller that got no generation is given.

use std::sync::atomic::Ordering;

use axum::response::{IntoResponse, Response};
use tracing::info;

use crate::core::server::admission::identity::ModelClientIdentity;
use crate::core::server::refusal::classed::{provider_refusal_contract, response_contract};
use crate::core::server::refusal::envelope::model_error_envelope;
use crate::core::server::refusal::error_response;
use crate::core::server::telemetry::{
    TOTAL_FAILURES, TOTAL_INPUT_TOKENS, TOTAL_OUTPUT_TOKENS, TOTAL_PROVIDER_ATTEMPTS,
    TOTAL_REQUESTS,
};
use crate::subscription_dispatch::RoutedStream;
use crate::types::{GatewayRefusal, ModelResponse, ProviderRefusal, Refusal};

use super::routing::RoutedCallMeta;

/// One finished call, as the process counters count it.
const ONE_CALL: u64 = std::num::NonZeroU64::MIN.get();

/// Fold one buffered answer into the process statistics and the request log.
pub(super) fn tally_and_log_buffered(
    resp: &ModelResponse,
    client_identity: &ModelClientIdentity,
    meta: &RoutedCallMeta,
    requested_model: &str,
) {
    TOTAL_REQUESTS.fetch_add(ONE_CALL, Ordering::Relaxed);
    if let Some(usage) = resp.usage {
        TOTAL_INPUT_TOKENS.fetch_add(u64::from(usage.input_tokens), Ordering::Relaxed);
        TOTAL_OUTPUT_TOKENS.fetch_add(u64::from(usage.output_tokens), Ordering::Relaxed);
    }
    TOTAL_PROVIDER_ATTEMPTS.fetch_add(resp.attempts as u64, Ordering::Relaxed);
    if !resp.success() {
        TOTAL_FAILURES.fetch_add(ONE_CALL, Ordering::Relaxed);
    }
    let failure_contract = response_contract(resp);
    crate::journal::answers::record(
        requested_model,
        failure_contract.map(|contract| (contract.code, resp.reason())),
    );
    // `error_code` below is Brama's own contract code, unchanged, because log
    // pipelines read it. `envelope` is the fleet's reading of the same failure;
    // a success has neither and says so.
    let (error_code, failure_envelope) = match (&resp.refusal, failure_contract) {
        (Some(refusal), Some(contract)) => (
            contract.code,
            model_error_envelope(&refusal.message, refusal.class),
        ),
        _ => ("none", "none".to_owned()),
    };
    info!(
        event = "routing_complete",
        request_id = %meta.request_id,
        client_id = %client_identity.client_id,
        routing_mode = meta.routing_mode,
        requested_model,
        selected_model = %resp.model,
        attempts = resp.attempts,
        success = resp.success(),
        streamed = false,
        elapsed_ms = meta.started.elapsed().as_millis() as u64,
        error_code,
        retryable = failure_contract.is_some_and(|contract| contract.retryable),
        input_tokens = resp.usage.map(|usage| usage.input_tokens),
        output_tokens = resp.usage.map(|usage| usage.output_tokens),
        operator_action_required = failure_contract.is_some_and(|contract| !contract.retryable),
        envelope = %failure_envelope,
        "routing request completed"
    );
}

/// Shape one refused call as this gateway's normalized error document.
///
/// Every format shares it: a caller that cannot get its generation needs the
/// contract code and retryability, and those are Brama's, not the wire's.
pub(super) fn failure_response(refusal: Refusal, attempts: u32) -> Response {
    // Refused for want of a signed-in subscription: the gateway's own
    // maintenance pass is what signs one in, so the refusal starts it.
    if refusal.class
        == ProviderRefusal::Gateway(GatewayRefusal::SubscriptionReauthorizationRequired)
    {
        crate::core::server::subscriptions::maintenance::start_after_refusal();
    }
    let contract = provider_refusal_contract(refusal.class);
    error_response(
        contract.status,
        contract.error_type,
        contract.code,
        &refusal.message,
        contract.retryable,
        Some(attempts),
    )
    .into_response()
}

/// Record that a stream committed: statistics that are already final, and the
/// one log line that says a generation is on its way.
pub(super) fn log_stream_commit(
    routed: &RoutedStream,
    client_identity: &ModelClientIdentity,
    meta: &RoutedCallMeta,
    requested_model: &str,
) {
    TOTAL_REQUESTS.fetch_add(ONE_CALL, Ordering::Relaxed);
    TOTAL_PROVIDER_ATTEMPTS.fetch_add(u64::from(routed.attempts), Ordering::Relaxed);
    crate::journal::answers::record(requested_model, None);
    info!(
        event = "routing_complete",
        request_id = %meta.request_id,
        client_id = %client_identity.client_id,
        routing_mode = meta.routing_mode,
        requested_model,
        selected_model = %routed.model,
        attempts = routed.attempts,
        success = true,
        streamed = true,
        elapsed_ms = meta.started.elapsed().as_millis() as u64,
        error_code = "none",
        "streaming request committed"
    );
}
