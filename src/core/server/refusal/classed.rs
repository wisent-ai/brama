//! The contract a refusal is answered with, from its class: provider refusals
//! from their status, Brama's own from the class stated where they were built.
//! No refusal is answered from its sentence.

use axum::http::StatusCode;

use super::contract::ModelErrorContract;
use crate::types::{GatewayRefusal, ModelResponse, ProviderRefusal};

/// The class of one failed answer. Every failure states one; a response that
/// arrived failed without it was built by no path this gateway has, and reads
/// as the unattributed provider failure it would otherwise be guessed as.
pub fn failure_class(response: &ModelResponse) -> Option<ProviderRefusal> {
    (!response.success).then(|| {
        response
            .failure_kind
            .unwrap_or(ProviderRefusal::ProviderFailure)
    })
}

/// The contract for one failed answer, from its class.
pub fn response_contract(response: &ModelResponse) -> Option<ModelErrorContract> {
    failure_class(response).map(provider_refusal_contract)
}

/// The contract for a refusal, from its class.
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
        ProviderRefusal::DependencyTimeout => ModelErrorContract {
            status: StatusCode::GATEWAY_TIMEOUT,
            error_type: "dependency_error",
            code: "dependency_timeout",
            retryable: true,
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
