//! Which single answer is worth spending a request on when a provider
//! publishes nothing for free.

use super::super::registry::route;

/// The model each provider's plan state is cheapest to ask for, when an
/// operator asks for it.
///
/// A provider states its plan windows in the headers of an ordinary completion,
/// so learning them this way costs one completion. Which model that completion
/// names changes the price and nothing else, so the smallest one the provider
/// offers is named here; the entry is the provider's own cheapest, not a default
/// a caller would ever be routed to. Nothing spends this on a timer: the free
/// usage reports declared in `PLAN_USAGE_ENDPOINTS` are what keeps a row
/// current, and this table only serves the on-demand check an operator
/// triggers.
const PLAN_PROBE_MODELS: &[(&str, &str)] = &[
    ("anthropic", "claude-haiku-4-5"),
    ("claude-code", "claude-haiku-4-5"),
    ("codex", "gpt-5.3-codex-spark"),
    ("kimi", "kimi-for-coding"),
    ("deepseek", "deepseek-chat"),
    ("openrouter", "openai/gpt-4o-mini"),
];

/// The route to spend one request on to learn a provider's plan state, when
/// there is one worth spending.
pub fn plan_probe_route(provider_id: &str) -> Option<String> {
    let model_id = PLAN_PROBE_MODELS
        .iter()
        .find(|(candidate, _)| *candidate == provider_id)
        .map(|(_, model_id)| *model_id)?;
    let candidate = format!("{provider_id}/{model_id}");
    // Built from a table rather than parsed from a caller, and still checked:
    // a typo here would otherwise reach a provider as a model it does not have.
    route(&candidate).is_some().then_some(candidate)
}
