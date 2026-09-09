//! How a refusal is spelled to a caller.
//!
//! One document shape for every path in this gateway: a sentence, the type, the
//! contract code, whether waiting can help, and how many providers were asked.
//! [`contract`] classifies a provider's or a dependency's own refusal sentence
//! into that shape; [`envelope`] additionally records the fleet's reading of the
//! same failure in the log, where a new key is not a wire change.

pub(in crate::core::server) mod contract;
pub(in crate::core::server) mod envelope;

use axum::http::StatusCode;
use axum::Json;
use serde_json::json;

pub(in crate::core::server) type ApiError = (StatusCode, Json<serde_json::Value>);

pub(in crate::core::server) fn error_response(
    status: StatusCode,
    error_type: &str,
    code: &str,
    message: &str,
    retryable: bool,
    attempts: u32,
) -> ApiError {
    (
        status,
        Json(json!({
            "error": {
                "message": message,
                "type": error_type,
                "code": code,
                "retryable": retryable,
                "attempts": attempts,
            }
        })),
    )
}

pub(in crate::core::server) fn api_error(status: StatusCode, message: &str) -> ApiError {
    let (error_type, code, retryable) = match status {
        StatusCode::BAD_REQUEST => ("request_error", "invalid_request", false),
        StatusCode::UNAUTHORIZED => ("authentication_error", "unauthenticated", false),
        StatusCode::FORBIDDEN => ("authorization_error", "forbidden", false),
        StatusCode::NOT_FOUND => ("state_error", "subscription_not_found", false),
        StatusCode::CONFLICT => ("state_error", "state_conflict", false),
        StatusCode::UPGRADE_REQUIRED => ("transport_error", "secure_transport_required", false),
        StatusCode::TOO_MANY_REQUESTS => ("capacity_error", "subscription_unavailable", true),
        StatusCode::SERVICE_UNAVAILABLE => ("dependency_error", "dependency_unavailable", true),
        StatusCode::GATEWAY_TIMEOUT => ("dependency_error", "dependency_timeout", true),
        StatusCode::BAD_GATEWAY => ("provider_error", "provider_failure", false),
        _ => ("internal_error", "internal_error", false),
    };
    error_response(status, error_type, code, message, retryable, u32::default())
}

/// An error whose code is the fault itself, with the facts a caller needs to
/// act on it beside the sentence. `api_error` picks the code from the status,
/// which is right when the status is the whole story and wrong when the same
/// 503 can mean "provider down" or "this alias was never wired up".
pub(in crate::core::server) fn api_error_with_details(
    status: StatusCode,
    code: &str,
    message: &str,
    details: serde_json::Value,
) -> ApiError {
    let error_type = match status {
        StatusCode::SERVICE_UNAVAILABLE | StatusCode::GATEWAY_TIMEOUT => "dependency_error",
        StatusCode::BAD_REQUEST => "request_error",
        _ => "state_error",
    };
    (
        status,
        Json(json!({
            "error": {
                "message": message,
                "type": error_type,
                "code": code,
                "retryable": false,
                "attempts": u32::default(),
                "details": details,
            }
        })),
    )
}
