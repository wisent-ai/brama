//! Which single answer is worth spending a request on when a provider
//! publishes nothing for free.

use super::super::registry::{provider, route};

/// A small completion on a model available to the provider's subscription:
/// the first model the provider's descriptor lists, which every roster orders
/// plan-compatible first.
///
/// A cheaper model is not necessarily part of the same plan. Codex Spark
/// refused imported ChatGPT grants that successfully served GPT-6-Astra,
/// making valid imports look like authentication failures, which is why the
/// descriptor's own first model is used rather than a second list of probe
/// models kept here. A provider that lists no static model has nothing to
/// probe. Nothing spends this on a timer: free usage reports keep rows current.
pub fn plan_probe_route(provider_id: &str) -> Option<String> {
    let model_id = provider(provider_id)?.static_models.first()?;
    let candidate = format!("{provider_id}/{model_id}");
    // Built from the roster rather than parsed from a caller, and still
    // checked: a typo there would otherwise reach a provider as a model it
    // does not have.
    route(&candidate).is_some().then_some(candidate)
}
