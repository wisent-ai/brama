//! Who a request is from: the identity every handler downstream is given, and
//! the two shapes it comes in — a workload that may carry an agent, and a
//! human whose organization was authorized for this request.

use std::collections::HashSet;

use serde::Deserialize;

pub(in crate::core::server) const BRAMA_DESKTOP_CLIENT_ID: &str = "brama-desktop";
pub(in crate::core::server) const BRAMA_USER_CLIENT_ID: &str = "brama-user";

/// The client a standalone launcher names as its own gateway's console: a
/// private gateway's launcher (Jeden's) administers it as Brama Desktop
/// administers its own. Read only in a standalone process; a managed
/// gateway's console is Brama Desktop alone.
const STANDALONE_CONSOLE_ENV: &str = "BRAMA_STANDALONE_CONSOLE_CLIENT";

/// Whether `client_id` is this gateway's console: Brama Desktop, or, in a
/// standalone process, the client its launcher names in
/// `BRAMA_STANDALONE_CONSOLE_CLIENT`. A declaration that cannot be read is
/// refused, never taken for no console.
pub(in crate::core::server) fn is_console(client_id: &str) -> Result<bool, String> {
    if client_id == BRAMA_DESKTOP_CLIENT_ID {
        return Ok(true);
    }
    if !crate::gateway::broker::local_provider_credentials_enabled() {
        return Ok(false);
    }
    Ok(
        crate::environment::declared_setting(STANDALONE_CONSOLE_ENV)?
            .is_some_and(|named| named == client_id),
    )
}

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
    /// The credential was issued the whole service (`call:brama` with no
    /// route): every model is authorized. It stays model-scoped for the
    /// paths it may reach, so it never stands for the console's identity,
    /// which carries no allowlist at all.
    pub(in crate::core::server) every_model: bool,
}

/// The Skarbiec capability that issues one route, and the one that issues
/// every route of this service.
const ROUTE_CAPABILITY: &str = "call:brama#";
const SERVICE_CAPABILITY: &str = "call:brama";

impl ModelClientIdentity {
    pub(in crate::core::server) fn authorizes_model(&self, model: &str) -> bool {
        self.every_model
            || self
                .allowed_models
                .as_ref()
                .is_none_or(|models| models.contains(model))
    }

    /// The refusal of `model` to this client: who asked, which model, the
    /// models its credential is scoped to, the capability it lacks and, for
    /// a workload credential, the command that issues it.
    pub(in crate::core::server) fn model_refusal(&self, model: &str) -> String {
        let mut allowed: Vec<&str> = self
            .allowed_models
            .iter()
            .flatten()
            .map(String::as_str)
            .collect();
        allowed.sort_unstable();
        let scoped = format!(
            "client {} may not use model {model}: its credential is scoped to {}",
            self.client_id,
            if allowed.is_empty() {
                "no model".to_string()
            } else {
                allowed.join(", ")
            }
        );
        match &self.kind {
            ModelClientKind::Workload { agent_id } => format!(
                "{scoped}. It lacks the Skarbiec capability {ROUTE_CAPABILITY}{model}; \
                 {SERVICE_CAPABILITY} issues every route. On the vault owner: stado credentials \
                 token mint {} --host <vault owner> --capabilities {SERVICE_CAPABILITY},<the \
                 consumer's other capabilities> --audience {} --replace-capabilities --token-item \
                 <the item holding its bearer>",
                self.client_id,
                agent_id.as_deref().unwrap_or(&self.client_id)
            ),
            ModelClientKind::Human(_) => scoped,
        }
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
