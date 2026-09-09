//! Naming a refusal: the contract kind, the provider's own sentence, the
//! transport cause, and the envelope that goes in the log.

use serde_json::Value;
use tracing::warn;
use wisent_errors::Code;

use crate::core::failure::{self, IMPACT_MODEL_REQUEST, POINT_PROVIDER_CALL};
use crate::types::ModelResponse;

fn max_provider_error_chars() -> usize {
    "2048"
        .parse()
        .expect("valid provider error character limit")
}

/// Characters of transport cause a failure sentence may carry. Long enough for
/// a chain like "error sending request: connection refused", short enough that
/// a log line survives whole.
const MAX_TRANSPORT_CAUSE: usize = 300;

/// The reason a send failed, in the caller's vocabulary and with the cause.
///
/// The bare sentences this used to return said a request failed and nothing
/// else. On 2026-09-05 every product chat failed with `dependency_unavailable:
/// provider request failed` while the model host answered other callers in
/// 0.3 s, and the gateway's own log could not distinguish a refused
/// connection, an unroutable address and a TLS refusal — so an outage that was
/// entirely inside one hop took hours to name. The transport error carries that
/// answer already; withholding it was the defect.
///
/// The cause is bounded and carries no request body, only the client's own
/// description of why the socket did not carry the call.
pub(in crate::providers::adapter) fn transport_error_message(error: &reqwest::Error) -> String {
    let mut cause = error.to_string();
    let mut source = std::error::Error::source(error);
    while let Some(inner) = source {
        cause.push_str(": ");
        cause.push_str(&inner.to_string());
        source = std::error::Error::source(inner);
    }
    let cause: String = cause.chars().take(MAX_TRANSPORT_CAUSE).collect();
    if error.is_timeout() {
        format!("dependency_timeout: provider request timed out: {cause}")
    } else {
        format!("dependency_unavailable: provider request failed: {cause}")
    }
}

pub(in crate::providers::adapter) fn transport_failure(
    route_id: &str,
    error: &reqwest::Error,
) -> ModelResponse {
    attempted_failure(route_id, transport_error_message(error))
}

pub(in crate::providers::adapter) fn attempted_failure(
    route_id: &str,
    message: String,
) -> ModelResponse {
    let mut failure = ModelResponse::failure(route_id, message);
    failure.attempts = u32::from(true);
    failure
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
    let detail = detail
        .chars()
        .take(max_provider_error_chars())
        .collect::<String>();
    // The kind is what clients read, so it is exactly what it has always been,
    // 404, 407, 408 and 410 attributed to nothing and 504 called an unreachable
    // dependency included.
    let kind = if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        "provider_rate_limited"
    } else if matches!(
        status,
        reqwest::StatusCode::UNAUTHORIZED | reqwest::StatusCode::FORBIDDEN
    ) {
        "provider_authentication"
    } else if status.is_server_error() {
        "dependency_unavailable"
    } else {
        "provider_failure"
    };
    (kind, detail)
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
    attempted_failure(route_id, format!("{kind}: {detail}"))
}
