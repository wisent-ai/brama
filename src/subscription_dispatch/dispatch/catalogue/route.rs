//! Which provider owns a canonical `provider/model` route, and whether that
//! provider is one only a signed caller may be billed to.

use crate::providers::adapter as provider_registry;

pub fn is_subscription_model(model: &str) -> bool {
    provider_for(model).is_some()
}

pub fn provider_requires_caller_identity(model: &str) -> bool {
    matches!(provider_for(model), Some("claude-code" | "codex" | "kimi"))
}

pub(crate) fn provider_for(model: &str) -> Option<&str> {
    provider_registry::provider_id_from_route(model)
}

pub(in crate::subscription_dispatch::dispatch) fn provider_matches(
    candidate: &str,
    requested: &str,
) -> bool {
    candidate.trim().eq_ignore_ascii_case(requested)
}
