//! The same failure in the fleet's envelope, and the refusal a typed call
//! answers with.

use crate::core::failure::{self, IMPACT_MODEL_REQUEST, POINT_MODEL_REQUEST};

use super::contract::{model_error_contract, ModelErrorContract};
use super::{error_response, ApiError};

/// The same failure in the fleet's envelope, for the operator reading the log.
///
/// The contract is what clients read and nothing here touches it. The envelope
/// is additional and it stays in the log: a new key in the HTTP error body is a
/// wire change, and a migration whose whole promise is "no behaviour change"
/// does not get to make one. Where the two disagree -- the contract is coarser
/// at a handful of provider statuses -- both are on the line, so an operator
/// can see that they do.
pub(in crate::core::server) fn model_error_envelope(
    message: &str,
    contract: ModelErrorContract,
) -> String {
    failure::envelope(
        POINT_MODEL_REQUEST,
        failure::code_for_message(message, contract.code),
        IMPACT_MODEL_REQUEST,
        message,
    )
    .to_json()
}

pub(in crate::core::server) fn typed_dispatch_attempts(contract: ModelErrorContract) -> u32 {
    if contract.code == "dependency_unavailable" {
        u32::default()
    } else {
        u32::from(true)
    }
}

pub(in crate::core::server) fn typed_dispatch_error(message: &str) -> ApiError {
    let contract = model_error_contract(message);
    let attempts = typed_dispatch_attempts(contract);
    error_response(
        contract.status,
        contract.error_type,
        contract.code,
        message,
        contract.retryable,
        attempts,
    )
}
