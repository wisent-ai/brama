//! Which single answer is worth spending a request on when a provider
//! publishes nothing for free.

use super::super::registry::route;

/// A small completion on a model available to the provider's subscription.
///
/// A cheaper model is not necessarily part of the same plan. Codex Spark
/// refused imported ChatGPT grants that successfully served GPT-6-Astra,
/// making valid imports look like authentication failures. The probe uses
/// the plan-compatible route instead; its output budget remains minimal.
/// Nothing spends this on a timer: free usage reports keep rows current.
const PLAN_PROBE_MODELS: &[(&str, &str)] = &[
    ("anthropic", "claude-haiku-4-5"),
    ("claude-code", "claude-haiku-4-5"),
    ("codex", "gpt-6-astra"),
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
