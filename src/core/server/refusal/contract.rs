//! The contract a client is answered with for one refusal. Every class maps
//! onto one in [`super::classed::provider_refusal_contract`]; the rule the
//! mapping keeps is that an authorization failure is never dressed as
//! capacity, because a caller told to wait retries against a credential
//! nobody will reissue by waiting.

use axum::http::StatusCode;

/// What Brama answers a client with for one refusal class.
///
/// Public because it is a contract, not an implementation detail: the status,
/// the type, the code and `retryable` are what every caller in the fleet acts
/// on, and ARCHITECTURE.md records the rule they must obey.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModelErrorContract {
    pub status: StatusCode,
    pub error_type: &'static str,
    pub code: &'static str,
    pub retryable: bool,
}
