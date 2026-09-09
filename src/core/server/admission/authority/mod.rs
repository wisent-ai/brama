//! Resolving a bearer this process was not started with.
//!
//! The boot table is a copy of the vault taken at start: it cannot expire, it
//! cannot be revoked, and a client registered since is absent from it. So a
//! bearer that is not in it is put to the authority that issued it —
//! [`skarbiec`] for a workload, [`wisent`] for a person — and the answer is
//! remembered for a few seconds, keyed by identity namespace as well as digest.

pub(super) mod skarbiec;
pub(super) mod wisent;

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use axum::http::HeaderMap;
use sha2::{Digest, Sha256};

use super::identity::{ModelClientIdentity, ModelClientKind, BRAMA_USER_CLIENT_ID};
use super::presented_bearer;

pub(in crate::core::server) const WISENT_ORGANIZATION_HEADER: &str = "x-wisent-organization-id";

/// A cache key carries the identity namespace as well as the bearer digest.
/// Workload credentials have no organization context; a human entry is valid
/// only for the organization that Supabase authorized for that request.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
enum IdentityCacheKey {
    Workload(String),
    Human {
        token_digest: String,
        organization_id: uuid::Uuid,
    },
}

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
    let digest = hex::encode(Sha256::digest(bearer.as_bytes()));
    let workload_key = IdentityCacheKey::Workload(digest.clone());
    if let Some(identity) = cached_identity(&workload_key) {
        return Ok(identity);
    }

    let organization_header = exact_organization_header(headers);
    if let Ok(Some(organization_id)) = organization_header {
        let human_key = IdentityCacheKey::Human {
            token_digest: digest.clone(),
            organization_id,
        };
        if let Some(identity) = cached_identity(&human_key) {
            return Ok(identity);
        }
    }

    let workload_unavailable = match skarbiec::ask_authority(bearer).await {
        WorkloadAuthorityAnswer::Resolved(identity) => {
            remember_identity(workload_key, identity.clone());
            return Ok(identity);
        }
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
    let identity = ModelClientIdentity {
        client_id: BRAMA_USER_CLIENT_ID.to_string(),
        kind: ModelClientKind::Human(context),
        // Human model access continues to come from the user's own stored
        // subscriptions, never from deployment or workload capabilities.
        allowed_models: Some(HashSet::new()),
    };
    remember_identity(
        IdentityCacheKey::Human {
            token_digest: digest,
            organization_id,
        },
        identity.clone(),
    );
    Ok(identity)
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

type IdentityCache =
    std::sync::Mutex<HashMap<IdentityCacheKey, (std::time::Instant, ModelClientIdentity)>>;
static IDENTITY_CACHE: LazyLock<IdentityCache> = LazyLock::new(Default::default);

fn identity_cache_ttl() -> std::time::Duration {
    std::time::Duration::from_secs("5".parse().expect("static number"))
}

fn cached_identity(key: &IdentityCacheKey) -> Option<ModelClientIdentity> {
    let cache = &*IDENTITY_CACHE;
    let guard = cache.lock().ok()?;
    let (seen, identity) = guard.get(key)?;
    if seen.elapsed() > identity_cache_ttl() {
        return None;
    }
    Some(identity.clone())
}

fn remember_identity(key: IdentityCacheKey, identity: ModelClientIdentity) {
    let cache = &*IDENTITY_CACHE;
    if let Ok(mut guard) = cache.lock() {
        let ttl = identity_cache_ttl();
        guard.retain(|_, (seen, _)| seen.elapsed() <= ttl);
        guard.insert(key, (std::time::Instant::now(), identity));
    }
}
