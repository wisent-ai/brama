//! Resolving a bearer this process was not started with.
//!
//! The boot table is a copy of the vault taken at start: it cannot expire, it
//! cannot be revoked, and a client registered since is absent from it. So a
//! bearer that is not in it is put to the authority that issued it —
//! [`skarbiec`] for a workload, [`wisent`] for a person — on every request,
//! so a revocation takes effect on the next request.

pub(super) mod skarbiec;
pub(super) mod wisent;

use std::collections::HashSet;

use axum::http::HeaderMap;

use super::identity::{ModelClientIdentity, ModelClientKind, BRAMA_USER_CLIENT_ID};
use super::presented_bearer;

pub(in crate::core::server) const WISENT_ORGANIZATION_HEADER: &str = "x-wisent-organization-id";

#[derive(Clone, Copy, Debug)]
pub(super) enum IdentityResolutionError {
    InvalidOrganizationHeader,
    Forbidden,
    Unauthorized,
    UpstreamUnavailable,
}

pub(super) enum WorkloadAuthorityAnswer {
    Resolved(ModelClientIdentity),
    Rejected,
    NotConfigured,
    Unavailable,
}

pub(super) enum WisentIdentityAnswer {
    Resolved(uuid::Uuid),
    Rejected,
    Unavailable,
}

/// Resolve workload credentials through their service authority and human
/// credentials through canonical Wisent Supabase. A human is not an identity
/// until the requested organization has been authorized with that same bearer.
pub(super) async fn identity_from_authority(
    headers: &HeaderMap,
) -> Result<ModelClientIdentity, IdentityResolutionError> {
    let bearer = presented_bearer(headers).ok_or(IdentityResolutionError::Unauthorized)?;
    let organization_header = exact_organization_header(headers);
    let workload_unavailable = match skarbiec::ask_authority(bearer).await {
        WorkloadAuthorityAnswer::Resolved(identity) => return Ok(identity),
        WorkloadAuthorityAnswer::Unavailable => true,
        WorkloadAuthorityAnswer::Rejected | WorkloadAuthorityAnswer::NotConfigured => false,
    };

    let user_id = match wisent::ask_wisent_identity(bearer).await {
        WisentIdentityAnswer::Resolved(user_id) => user_id,
        WisentIdentityAnswer::Rejected if workload_unavailable => {
            return Err(IdentityResolutionError::UpstreamUnavailable)
        }
        WisentIdentityAnswer::Rejected => return Err(IdentityResolutionError::Unauthorized),
        WisentIdentityAnswer::Unavailable => {
            return Err(IdentityResolutionError::UpstreamUnavailable)
        }
    };
    let organization_id = organization_header
        .map_err(|_| IdentityResolutionError::InvalidOrganizationHeader)?
        .ok_or(IdentityResolutionError::InvalidOrganizationHeader)?;
    let context = wisent::authorize_organization(bearer, user_id, organization_id).await?;
    Ok(ModelClientIdentity {
        client_id: BRAMA_USER_CLIENT_ID.to_string(),
        kind: ModelClientKind::Human(context),
        // Human model access continues to come from the user's own stored
        // subscriptions, never from deployment or workload capabilities.
        allowed_models: Some(HashSet::new()),
    })
}

fn exact_organization_header(
    headers: &HeaderMap,
) -> Result<Option<uuid::Uuid>, IdentityResolutionError> {
    let mut values = headers.get_all(WISENT_ORGANIZATION_HEADER).iter();
    let Some(value) = values.next() else {
        return Ok(None);
    };
    if values.next().is_some() {
        return Err(IdentityResolutionError::InvalidOrganizationHeader);
    }
    let value = value
        .to_str()
        .map_err(|_| IdentityResolutionError::InvalidOrganizationHeader)?;
    uuid::Uuid::parse_str(value)
        .map(Some)
        .map_err(|_| IdentityResolutionError::InvalidOrganizationHeader)
}
