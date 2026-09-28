//! The class of a refused model request, stated where the refusal is built so
//! rotation and the HTTP edge decide from it and never from the sentence.

use serde::{Deserialize, Serialize};

/// How a provider refused one call, from the status it answered with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderRefusal {
    /// 429: the credential's quota or rate window is spent.
    RateLimited,
    /// 429 whose error object says `insufficient_quota`: the account's paid
    /// balance is spent. Another account may serve; waiting does not.
    QuotaExhausted,
    /// 401 or 403: the provider does not accept this credential.
    Authentication,
    /// 5xx: the provider could not answer.
    DependencyUnavailable,
    /// Any other status: this request was refused, not the credential.
    ProviderFailure,
    /// Brama refused the request itself before or instead of asking a provider.
    Gateway(GatewayRefusal),
}

/// Why Brama refused a request on its own account, stated where the refusal is
/// built so the HTTP edge answers from the class and never from the sentence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GatewayRefusal {
    /// The caller did not prove who it is.
    Unauthenticated,
    /// The route, selector or shape asked for does not exist here.
    InvalidRequest,
    /// Something Brama depends on (catalog, credential source) did not answer.
    DependencyUnavailable,
    /// The route cannot be served, for a reason the caller cannot repair.
    ProviderFailure,
    /// No credential could be produced for the agent: a capability, grant or
    /// active account is missing, and only an operator repairs it.
    CredentialUnauthorized,
    /// The provider refused every credential of the pool; a sign-in repairs it.
    SubscriptionReauthorizationRequired,
    /// Every usable credential is inside a quota block; waiting repairs it.
    SubscriptionUnavailable,
}

impl ProviderRefusal {
    /// The contract kind clients read in the refusal sentence.
    pub fn contract_kind(self) -> &'static str {
        match self {
            Self::RateLimited => "provider_rate_limited",
            Self::QuotaExhausted => "provider_quota_exhausted",
            Self::Authentication => "provider_authentication",
            Self::DependencyUnavailable | Self::Gateway(GatewayRefusal::DependencyUnavailable) => {
                "dependency_unavailable"
            }
            Self::ProviderFailure | Self::Gateway(GatewayRefusal::ProviderFailure) => {
                "provider_failure"
            }
            Self::Gateway(GatewayRefusal::Unauthenticated) => "unauthenticated",
            Self::Gateway(GatewayRefusal::InvalidRequest) => "invalid_request",
            Self::Gateway(GatewayRefusal::CredentialUnauthorized) => "credential_unauthorized",
            Self::Gateway(GatewayRefusal::SubscriptionReauthorizationRequired) => {
                "subscription_reauthorization_required"
            }
            Self::Gateway(GatewayRefusal::SubscriptionUnavailable) => "subscription_unavailable",
        }
    }
}
