//! Naming a refusal: the contract kind, the provider's own sentence, the
//! transport cause, and the envelope that goes in the log.

use serde_json::Value;
use tracing::warn;
use wisent_errors::Code;

use crate::core::failure::{self, IMPACT_MODEL_REQUEST, POINT_PROVIDER_CALL};
use crate::types::{ModelResponse, ProviderRefusal, Refusal};

/// The reason a send failed, in the caller's vocabulary and with the cause.
///
/// The bare sentences this used to return said a request failed and nothing
/// else: every product chat failing with `dependency_unavailable: provider
/// request failed` while the model host answers other callers in a fraction
/// of a second, and a gateway log that cannot distinguish a refused
/// connection, an unroutable address and a TLS refusal, is an outage
/// entirely inside one hop that takes hours to name. The transport error
/// carries that answer already; withholding it was the defect.
///
/// The cause carries no request body, only the client's own description of
/// why the socket did not carry the call.
pub(in crate::providers::adapter) fn transport_refusal(error: &reqwest::Error) -> Refusal {
    let cause = failure::error_chain(error);
    let (class, said) = if error.is_timeout() {
        (
            ProviderRefusal::DependencyTimeout,
            "provider request timed out",
        )
    } else {
        (
            ProviderRefusal::DependencyUnavailable,
            "provider request failed",
        )
    };
    Refusal::new(class, format!("{}: {said}: {cause}", class.contract_kind()))
}

pub(in crate::providers::adapter) fn transport_failure(
    route_id: &str,
    error: &reqwest::Error,
) -> ModelResponse {
    attempted_failure(route_id, transport_refusal(error))
}

pub(in crate::providers::adapter) fn attempted_failure(
    route_id: &str,
    refusal: Refusal,
) -> ModelResponse {
    let mut failure = ModelResponse::from_refusal(route_id, refusal);
    failure.attempts = 1;
    failure
}

/// A provider's refusal as a typed call carries it, with the class read from
/// the provider's status and the provider's sentence behind its kind.
pub(in crate::providers::adapter) fn provider_refused(
    route_id: &str,
    status: reqwest::StatusCode,
    body: &str,
) -> Refusal {
    let failure = provider_error(route_id, status, body);
    Refusal::new(
        failure
            .failure_kind
            .unwrap_or(ProviderRefusal::ProviderFailure),
        failure.error.unwrap_or_default(),
    )
}

/// Classify one refused provider answer: the contract kind clients read, and the
/// provider's own sentence, bounded like every other stored reason.
///
/// Shared by the model request path and the usage report reader so a credential
/// the provider refuses reads the same either way. A reader that acts on
/// `provider_authentication` -- the renewal loop does -- must not have to know
/// which of the two calls noticed first.
pub(in crate::providers::adapter) fn provider_refusal(
    status: reqwest::StatusCode,
    body: &str,
) -> (&'static str, String) {
    let detail = serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|value| {
            value
                .pointer("/error/message")
                .or_else(|| value.get("message"))
                .or_else(|| value.get("detail"))
                .or_else(|| value.get("error"))
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .or_else(|| {
            let detail = body.trim();
            (!detail.is_empty()).then(|| detail.to_string())
        })
        .unwrap_or_else(|| format!("provider returned HTTP {}", status.as_u16()));
    (refusal_class(status, body).contract_kind(), detail)
}

/// The OpenAI-shape error code for a spent paid balance, read from the error
/// object's `code` or `type` field.
const INSUFFICIENT_QUOTA: &str = "insufficient_quota";

/// The OpenAI-shape error code for a prompt longer than the model's context,
/// read from the error object's `code` or `type` field.
const CONTEXT_LENGTH_EXCEEDED: &str = "context_length_exceeded";

/// Whether the error object's `code` or `type` field names `wanted`.
fn error_names(body: &str, wanted: &str) -> bool {
    serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|value| value.get("error").cloned())
        .is_some_and(|error| {
            ["code", "type"]
                .iter()
                .any(|field| error.get(field).and_then(Value::as_str) == Some(wanted))
        })
}

/// The class of a refused answer, from its status, and for a 429 from the error
/// object's code, which is the only thing that tells a spent balance from a
/// rate window. A 413, or a client error whose error object names
/// `context_length_exceeded`, is a prompt the model cannot take, so a client can
/// promote the route or compact instead of failing the turn. The kind clients
/// read otherwise is exactly what it has always been, 404, 407, 408 and 410
/// attributed to nothing and 504 called an unreachable dependency included.
pub(in crate::providers::adapter) fn refusal_class(
    status: reqwest::StatusCode,
    body: &str,
) -> ProviderRefusal {
    if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        if error_names(body, INSUFFICIENT_QUOTA) {
            ProviderRefusal::QuotaExhausted
        } else {
            ProviderRefusal::RateLimited
        }
    } else if status == reqwest::StatusCode::PAYLOAD_TOO_LARGE
        || (status.is_client_error() && error_names(body, CONTEXT_LENGTH_EXCEEDED))
    {
        ProviderRefusal::ContextLengthExceeded
    } else if matches!(
        status,
        reqwest::StatusCode::UNAUTHORIZED | reqwest::StatusCode::FORBIDDEN
    ) {
        ProviderRefusal::Authentication
    } else if status.is_server_error() {
        ProviderRefusal::DependencyUnavailable
    } else {
        ProviderRefusal::ProviderFailure
    }
}

pub(in crate::providers::adapter) fn provider_error(
    route_id: &str,
    status: reqwest::StatusCode,
    body: &str,
) -> ModelResponse {
    let (kind, detail) = provider_refusal(status, body);
    // The envelope code is the fleet's, classified from the status by the
    // catalogue, so `error_code` means the same thing here as everywhere else.
    // It is coarser in Brama's kind at five statuses -- 404 and 410 are
    // not-found, 407 is auth, 408 and 504 are timeouts -- and both readings are
    // logged side by side rather than one quietly standing in for the other.
    let refused = failure::envelope(
        POINT_PROVIDER_CALL,
        Code::from_upstream_status(status.as_u16()),
        IMPACT_MODEL_REQUEST,
        detail.as_str(),
    )
    .with_context("route", route_id)
    .with_context("status", status.as_u16().to_string());
    warn!(
        event = "provider_rejected",
        route = route_id,
        contract_kind = kind,
        envelope = %refused.to_json(),
        "{}",
        refused.render()
    );
    attempted_failure(
        route_id,
        Refusal::new(refusal_class(status, body), format!("{kind}: {detail}")),
    )
}
