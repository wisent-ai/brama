//! Who a request is from: the identity every handler downstream is given, and
//! the two shapes it comes in — a workload that may carry an agent, and a
//! human whose organization was authorized for this request.

use std::collections::HashSet;

use serde::Deserialize;

pub(in crate::core::server) const BRAMA_DESKTOP_CLIENT_ID: &str = "brama-desktop";
pub(in crate::core::server) const BRAMA_USER_CLIENT_ID: &str = "brama-user";

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(in crate::core::server) enum OrganizationRole {
    Owner,
    Admin,
    Member,
}

#[derive(Clone, Debug)]
pub(in crate::core::server) struct HumanOrganizationContext {
    pub(in crate::core::server) user_id: uuid::Uuid,
    pub(in crate::core::server) organization_id: uuid::Uuid,
    pub(in crate::core::server) role: OrganizationRole,
}

#[derive(Clone, Debug)]
pub(in crate::core::server) enum ModelClientKind {
    Workload { agent_id: Option<String> },
    Human(HumanOrganizationContext),
}

#[derive(Clone, Debug)]
pub(in crate::core::server) struct ModelClientIdentity {
    pub(in crate::core::server) client_id: String,
    pub(in crate::core::server) kind: ModelClientKind,
    pub(in crate::core::server) allowed_models: Option<HashSet<String>>,
}

impl ModelClientIdentity {
    pub(in crate::core::server) fn authorizes_model(&self, model: &str) -> bool {
        self.allowed_models
            .as_ref()
            .is_none_or(|models| models.contains(model))
    }

    pub(in crate::core::server) fn agent_id(&self) -> Option<&str> {
        match &self.kind {
            ModelClientKind::Workload { agent_id } => agent_id.as_deref(),
            ModelClientKind::Human(_) => None,
        }
    }

    pub(in crate::core::server) fn human_context(&self) -> Option<&HumanOrganizationContext> {
        match &self.kind {
            ModelClientKind::Human(context) => Some(context),
            ModelClientKind::Workload { .. } => None,
        }
    }
}

pub(in crate::core::server) fn valid_agent_id(agent_id: &str) -> bool {
    !agent_id.is_empty()
        && agent_id
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}
