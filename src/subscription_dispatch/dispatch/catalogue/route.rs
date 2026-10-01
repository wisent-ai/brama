//! Which provider owns a canonical `provider/model` route, and whether that
//! provider is one only a signed caller may be billed to.

use crate::providers::adapter as provider_registry;

/// The providers whose credentials are subscriptions the pool holds, one
/// vault item per account, never one direct `provider:<name>` key. Spelled
/// once: asking Skarbiec for a bare `provider:claude-code` that several vault
/// items declare makes the vault refuse the ambiguity, and Stado then reads
/// that warning as the cause of repeated quarantines and stops promoting
/// Brama on that host.
pub const SUBSCRIPTION_PROVIDERS: [&str; 3] = ["claude-code", "codex", "kimi"];

pub fn is_subscription_provider(provider: &str) -> bool {
    SUBSCRIPTION_PROVIDERS.contains(&provider)
}

pub fn is_subscription_model(model: &str) -> bool {
    provider_for(model).is_some()
}

pub fn provider_requires_caller_identity(model: &str) -> bool {
    provider_for(model).is_some_and(is_subscription_provider)
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
