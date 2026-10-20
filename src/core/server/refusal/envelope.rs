//! The same failure in the fleet's envelope, and the refusal a typed call
//! answers with.

use crate::core::failure::{self, IMPACT_MODEL_REQUEST, POINT_MODEL_REQUEST};
use crate::types::{ProviderRefusal, Refusal};

use super::classed::provider_refusal_contract;
use super::{error_response, ApiError};

/// The same failure in the fleet's envelope, for the operator reading the log.
///
/// The contract is what clients read and nothing here touches it. The envelope
/// is additional and it stays in the log: a new key in the HTTP error body is a
/// wire change, and a migration whose whole promise is "no behaviour change"
/// does not get to make one. The envelope's code is the fleet's reading of the
/// refusal's class, which is finer than the contract at a few classes (an
/// authentication refusal and a provider failure share one contract code).
pub(in crate::core::server) fn model_error_envelope(
    message: &str,
    class: ProviderRefusal,
) -> String {
    failure::envelope(
        POINT_MODEL_REQUEST,
        failure::code_for(class.contract_kind()),
        IMPACT_MODEL_REQUEST,
        message,
    )
    .to_json()
}

/// A refused typed call: counted with the provider call it made, and answered
/// from its class.
pub(in crate::core::server) fn refused_typed(refused: &Refusal) -> ApiError {
    crate::core::server::telemetry::record_typed_refusal(refused);
    typed_dispatch_error(refused)
}

/// The refusal document one typed call answers with, from the class its
/// refusal was stated with. A typed call asks its provider once, so it states
/// one attempt, or none when Brama refused before asking.
pub(in crate::core::server) fn typed_dispatch_error(refused: &Refusal) -> ApiError {
    let contract = provider_refusal_contract(refused.class);
    error_response(
        contract.status,
        contract.error_type,
        contract.code,
        &refused.message,
        contract.retryable,
        refused
            .class
            .asked_provider()
            .then_some(std::num::NonZeroU32::MIN.get()),
    )
}
