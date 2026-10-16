//! Final-use capability redemption and the distinction between a refused
//! authorization, an unavailable authority, and unreadable local proof.

use crate::capability::{CapabilityError, CapabilityRef, Secret};
use crate::core::failure;
use wisent_errors::{Code, Failure};

use super::{configuration::client, credential_failure};

pub(super) async fn redeem_provider_resource(
    capability_id: &str,
    resource: &str,
) -> Result<Secret, Failure> {
    let id = capability_id.to_owned();
    let owned_resource = resource.to_owned();
    // Skarbiec uses blocking Unix I/O. Decryption must not occupy Tokio's
    // HTTP workers and starve /readyz during a credential sweep.
    tokio::task::spawn_blocking(move || {
        let binding = CapabilityRef::provider(&id, &owned_resource).map_err(|error| {
            credential_failure(
                format!("capability does not bind to `{owned_resource}`: {error}"),
                &owned_resource,
                Code::Config,
            )
        })?;
        let broker = client().map_err(|error| {
            let code = match &error {
                CapabilityError::KeyUnreadable(_) => Code::InfraDown,
                _ => Code::Config,
            };
            credential_failure(
                format!("construct capability client: {error}"),
                &owned_resource,
                code,
            )
        })?;
        broker.redeem(&binding).map_err(|error| {
            let code = match &error {
                CapabilityError::AuthorityUnreachable(_) => Code::InfraDown,
                CapabilityError::RedemptionDenied(_) => failure::code_for("credential_unauthorized"),
                _ => Code::Unknown,
            };
            credential_failure(format!("redeem capability: {error}"), &owned_resource, code)
        })
    })
    .await
    .map_err(|error| {
        credential_failure(
            format!("credential redemption worker failed: {error}"),
            resource,
            Code::Unknown,
        )
    })?
}
