//! The console's write surface: what the operator may change about this
//! gateway, and the proof required to change it.
//!
//! Every handler in this family begins with the same check — the caller is the
//! desktop console holding an unrestricted bearer — and then touches exactly
//! one thing: the route registry ([`routes`], [`adoption`]), the standalone
//! credential store ([`credentials`]), or nothing at all ([`snapshot`]).

pub(in crate::core::server) mod adoption;
pub(in crate::core::server) mod credentials;
pub(in crate::core::server) mod routes;
pub(in crate::core::server) mod snapshot;

use axum::http::StatusCode;
use serde::Deserialize;

use crate::core::server::admission::identity::{ModelClientIdentity, BRAMA_DESKTOP_CLIENT_ID};
use crate::core::server::aliases::{alias_requires_direct_capability, alias_route_shape_supported};
use crate::core::server::refusal::{api_error, ApiError};

pub(in crate::core::server) fn require_brama_desktop(
    identity: &ModelClientIdentity,
) -> Result<(), ApiError> {
    if identity.client_id == BRAMA_DESKTOP_CLIENT_ID && identity.allowed_models.is_none() {
        Ok(())
    } else {
        Err(api_error(StatusCode::FORBIDDEN, "forbidden"))
    }
}

pub(crate) fn valid_alias(alias: &str) -> bool {
    !alias.is_empty()
        && alias.len() <= 128
        && alias.trim() == alias
        && alias.bytes().all(|byte| {
            byte.is_ascii_lowercase()
                || byte.is_ascii_digit()
                || matches!(byte, b'-' | b'_' | b'.' | b'/')
        })
}

fn route_supported(alias: &str, route: &str) -> bool {
    if route.is_empty()
        || route.trim() != route
        || route.contains('*')
        || route.bytes().any(|byte| byte.is_ascii_control())
    {
        return false;
    }
    alias_route_shape_supported(alias, route)
        && (!alias_requires_direct_capability(alias, route)
            || crate::providers::adapter::provider_id_from_route(route)
                .is_some_and(crate::gateway::broker::provider_capability_configured))
}

/// One alias and the one route it points at. `deny_unknown_fields` is what
/// makes a field the registry no longer has a refusal rather than a dropped
/// intention, so a caller that still sends a ranked list is told so instead of
/// having the list quietly ignored.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::core::server) struct AdminRouteUpdate {
    pub(super) alias: String,
    pub(super) primary: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::core::server) struct AdminRouteDelete {
    pub(super) alias: String,
}

fn default_adoption_agent_id() -> String {
    "wisent-app".to_string()
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::core::server) struct AdminAdoptionPreview {
    pub(super) document: String,
    pub(super) source_name: String,
    #[serde(default = "default_adoption_agent_id")]
    pub(super) agent_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::core::server) struct AdminAdoptionApply {
    pub(super) document: String,
    pub(super) source_name: String,
    #[serde(default = "default_adoption_agent_id")]
    pub(super) agent_id: String,
    #[serde(default)]
    pub(super) selected_aliases: Vec<String>,
    #[serde(default)]
    pub(super) replace_alias_conflicts: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::core::server) struct AdminCredentialMutation {
    pub(super) provider: String,
    #[serde(default)]
    pub(super) credential: Option<String>,
}
