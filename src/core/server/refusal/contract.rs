//! One refusal sentence, classified into the contract a client is answered
//! with. Every arm below was written because a real caller acted on the wrong
//! one: an authorization failure dressed as capacity sends an agent into
//! retries against a credential nobody will reissue by waiting.

use axum::http::StatusCode;

/// What Brama answers a client with for one refusal sentence.
///
/// Public because it is a contract, not an implementation detail: the status,
/// the type, the code and `retryable` are what every caller in the fleet acts
/// on, ARCHITECTURE.md records the rule they must obey - an authorization
/// failure is never dressed as capacity - and it has now regressed twice while
/// being unreachable from any test.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModelErrorContract {
    pub status: StatusCode,
    pub error_type: &'static str,
    pub code: &'static str,
    pub retryable: bool,
}

/// Classify one refusal sentence into the contract a client is answered with.
pub fn model_error_contract(message: &str) -> ModelErrorContract {
    let normalized = message.to_ascii_lowercase();
    if normalized.contains("no credits remaining")
        || normalized.contains("insufficient_quota")
        || normalized.contains("exceeded your current quota")
        || normalized.contains("billing hard limit has been reached")
    {
        return ModelErrorContract {
            status: StatusCode::BAD_GATEWAY,
            error_type: "provider_error",
            code: "provider_quota_exhausted",
            retryable: false,
        };
    }
    if normalized.starts_with("provider_rate_limited:") {
        return ModelErrorContract {
            status: StatusCode::TOO_MANY_REQUESTS,
            error_type: "capacity_error",
            code: "provider_rate_limited",
            retryable: true,
        };
    }
    if normalized.starts_with("dependency_timeout:") {
        return ModelErrorContract {
            status: StatusCode::GATEWAY_TIMEOUT,
            error_type: "dependency_error",
            code: "dependency_timeout",
            retryable: true,
        };
    }
    if normalized.starts_with("dependency_unavailable:") {
        return ModelErrorContract {
            status: StatusCode::SERVICE_UNAVAILABLE,
            error_type: "dependency_error",
            code: "dependency_unavailable",
            retryable: true,
        };
    }
    if normalized.starts_with("provider_failure:")
        || normalized.starts_with("provider_authentication:")
    {
        return ModelErrorContract {
            status: StatusCode::BAD_GATEWAY,
            error_type: "provider_error",
            code: "provider_failure",
            retryable: false,
        };
    }
    if normalized.starts_with("auth:")
        || normalized.contains("missing x-agent-")
        || normalized.contains("no auth secret for agent")
    {
        return ModelErrorContract {
            status: StatusCode::UNAUTHORIZED,
            error_type: "authentication_error",
            code: "unauthenticated",
            retryable: false,
        };
    }
    if normalized.contains("rate_limit")
        || normalized.contains("429")
        || normalized.contains("hit your limit")
        || normalized.contains("usage limit")
        || normalized.contains("weekly limit")
    {
        return ModelErrorContract {
            status: StatusCode::TOO_MANY_REQUESTS,
            error_type: "capacity_error",
            code: "provider_rate_limited",
            retryable: true,
        };
    }
    // A refused redemption is not a busy provider. Skarbiec says "capability is
    // not issued, has expired, has no uses left, or its authorization id does
    // not match", and every one of those is a broken authorization chain that
    // no amount of waiting repairs. Reporting it as capacity told the caller to
    // retry and told the operator to look for a subscription, which is where a
    // whole day went before the log was read directly.
    if normalized.contains("redemption denied")
        || normalized.contains("authorization id does not match")
        || normalized.contains("capability is not issued")
        || normalized.contains("no uses left")
        || normalized.contains("capability redemption denied")
    {
        return ModelErrorContract {
            status: StatusCode::SERVICE_UNAVAILABLE,
            error_type: "authorization_error",
            code: "credential_unauthorized",
            retryable: false,
        };
    }
    // A pool that produced no credential at all is the same shape of failure one
    // layer earlier: no provider was asked, so there is no capacity to wait for.
    // The chain that would have produced the secret -- capability, read grant,
    // installation trust material -- is broken, and only an operator repairs it.
    if normalized.contains("could be redeemed for agent") {
        return ModelErrorContract {
            status: StatusCode::SERVICE_UNAVAILABLE,
            error_type: "authorization_error",
            code: "credential_unauthorized",
            retryable: false,
        };
    }
    // A pool the provider rejected is not a pool that is busy. Waiting cannot
    // reach it: somebody has to authorize the subscription again, and saying
    // `429 capacity_error, retryable: true` sent this workstation's agent into
    // retries for hours against a credential the provider had already burnt.
    if normalized.contains("re-authorization required") {
        return ModelErrorContract {
            status: StatusCode::SERVICE_UNAVAILABLE,
            error_type: "authorization_error",
            code: "subscription_reauthorization_required",
            retryable: false,
        };
    }
    // A pool with no eligible row at all is discovery, not capacity: the agent
    // has no active credential of this provider, which is what a subscription
    // whose vault item lost its `brama:agent:` tag produces, and what a retired
    // subscription produces. Waiting restores neither a tag nor a retirement.
    // Each sentence is matched whole rather than by a bare "no active", so that
    // "no active stateless provider models for signed agent" -- a catalogue
    // answer, not a credential one -- keeps its own classification. The pinned
    // variant reads "... for provider 'x' and agent", so it needs its own arm
    // rather than the credential-sentence one.
    if (normalized.contains("no active") && normalized.contains("credential for agent"))
        || normalized.contains("is not active for provider")
    {
        return ModelErrorContract {
            status: StatusCode::SERVICE_UNAVAILABLE,
            error_type: "authorization_error",
            code: "credential_unauthorized",
            retryable: false,
        };
    }
    if normalized.contains("no active")
        || normalized.contains("no working")
        || normalized.contains("all bounded")
        || normalized.contains("selected credential")
    {
        return ModelErrorContract {
            status: StatusCode::TOO_MANY_REQUESTS,
            error_type: "capacity_error",
            code: "subscription_unavailable",
            retryable: true,
        };
    }
    if normalized.contains("deadline") || normalized.contains("timed out") {
        return ModelErrorContract {
            status: StatusCode::GATEWAY_TIMEOUT,
            error_type: "dependency_error",
            code: "dependency_timeout",
            retryable: true,
        };
    }
    if normalized.contains("unavailable")
        || normalized.contains("catalog")
        || normalized.contains("no provider")
    {
        return ModelErrorContract {
            status: StatusCode::SERVICE_UNAVAILABLE,
            error_type: "dependency_error",
            code: "dependency_unavailable",
            retryable: true,
        };
    }
    if normalized.contains("unknown provider/model")
        || normalized.contains("invalid provider/model")
        || normalized.contains("unsupported selector")
        || normalized.contains("no quality checks")
        // Asking to stream a route this gateway can only buffer is a request
        // this gateway cannot serve, not a provider that failed: nothing
        // upstream was contacted, and `502 provider_failure` sent the caller
        // looking at a provider that never saw the request.
        || normalized.contains("streaming is supported for")
    {
        return ModelErrorContract {
            status: StatusCode::BAD_REQUEST,
            error_type: "request_error",
            code: "invalid_request",
            retryable: false,
        };
    }
    ModelErrorContract {
        status: StatusCode::BAD_GATEWAY,
        error_type: "provider_error",
        code: "provider_failure",
        retryable: false,
    }
}
