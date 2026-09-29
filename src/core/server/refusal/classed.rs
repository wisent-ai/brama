//! The contract a refusal is answered with when its class is known: provider
//! refusals from their status, Brama's own from the class stated where they
//! were built. Only a refusal without a class falls to its sentence.

use axum::http::StatusCode;

use super::contract::{model_error_contract, ModelErrorContract};
use crate::types::{GatewayRefusal, ModelResponse, ProviderRefusal};

/// The contract for one failed answer: from the refusal class when one was
/// stated, and only for a refusal without a class from its sentence.
pub fn response_contract(response: &ModelResponse) -> Option<ModelErrorContract> {
    match response.failure_kind {
        Some(kind) => Some(provider_refusal_contract(kind)),
        None => response.error.as_deref().map(model_error_contract),
    }
}

/// The contract for a refusal, from its class. The same answers the sentence
/// arms of `model_error_contract` give the same refusals, without reading the
/// sentence.
pub fn provider_refusal_contract(kind: ProviderRefusal) -> ModelErrorContract {
    match kind {
        ProviderRefusal::RateLimited => ModelErrorContract {
            status: StatusCode::TOO_MANY_REQUESTS,
            error_type: "capacity_error",
            code: "provider_rate_limited",
            retryable: true,
        },
        ProviderRefusal::QuotaExhausted => ModelErrorContract {
            status: StatusCode::BAD_GATEWAY,
            error_type: "provider_error",
            code: "provider_quota_exhausted",
            retryable: false,
        },
        ProviderRefusal::ContextLengthExceeded => ModelErrorContract {
            status: StatusCode::BAD_REQUEST,
            error_type: "request_error",
            code: "context_length_exceeded",
            retryable: false,
        },
        ProviderRefusal::DependencyUnavailable
        | ProviderRefusal::Gateway(GatewayRefusal::DependencyUnavailable) => ModelErrorContract {
            status: StatusCode::SERVICE_UNAVAILABLE,
            error_type: "dependency_error",
            code: "dependency_unavailable",
            retryable: true,
        },
        ProviderRefusal::Authentication
        | ProviderRefusal::ProviderFailure
        | ProviderRefusal::Gateway(GatewayRefusal::ProviderFailure) => ModelErrorContract {
            status: StatusCode::BAD_GATEWAY,
            error_type: "provider_error",
            code: "provider_failure",
            retryable: false,
        },
        ProviderRefusal::Gateway(GatewayRefusal::Unauthenticated) => ModelErrorContract {
            status: StatusCode::UNAUTHORIZED,
            error_type: "authentication_error",
            code: "unauthenticated",
            retryable: false,
        },
        ProviderRefusal::Gateway(GatewayRefusal::InvalidRequest) => ModelErrorContract {
            status: StatusCode::BAD_REQUEST,
            error_type: "request_error",
            code: "invalid_request",
            retryable: false,
        },
        ProviderRefusal::Gateway(GatewayRefusal::CredentialUnauthorized) => ModelErrorContract {
            status: StatusCode::SERVICE_UNAVAILABLE,
            error_type: "authorization_error",
            code: "credential_unauthorized",
            retryable: false,
        },
        ProviderRefusal::Gateway(GatewayRefusal::SubscriptionReauthorizationRequired) => {
            ModelErrorContract {
                status: StatusCode::SERVICE_UNAVAILABLE,
                error_type: "authorization_error",
                code: "subscription_reauthorization_required",
                retryable: false,
            }
        }
        ProviderRefusal::Gateway(GatewayRefusal::SubscriptionUnavailable) => ModelErrorContract {
            status: StatusCode::TOO_MANY_REQUESTS,
            error_type: "capacity_error",
            code: "subscription_unavailable",
            retryable: true,
        },
    }
}
