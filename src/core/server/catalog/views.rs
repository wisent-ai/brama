//! The two shapes the model list is answered in: the Jeden minimum schema a
//! console renders from, and the OpenAI-shaped list every SDK expects.
//!
//! Both are rendered from the same assembled facts, so a name that is missing
//! from one is missing from the other, and neither can quietly disagree about
//! whether an id is available here.

use std::collections::{HashMap, HashSet};

use serde_json::{json, Value};

use crate::core::server::telemetry::perf_json;
use crate::providers::adapter::RegistryModel;

/// Everything assembled about the caller's catalogue, before it is shaped.
pub(super) struct CatalogView {
    pub(super) model_ids: Vec<String>,
    pub(super) available: HashSet<String>,
    pub(super) registry_metadata: HashMap<String, RegistryModel>,
    pub(super) unavailable_reasons: HashMap<String, String>,
    /// The caller proved an identity, so "can this be served for you" and the
    /// latency history are answerable. Both are answers about the caller, so
    /// neither is given to an unknown one.
    pub(super) caller_known: bool,
}

pub(super) fn jeden(view: CatalogView, catalog_revision: String, degraded: bool) -> Value {
    let CatalogView {
        model_ids,
        available,
        registry_metadata,
        unavailable_reasons,
        caller_known,
    } = view;
    let models = model_ids
        .into_iter()
        .map(|id| {
            let registry = registry_metadata.get(&id);
            let input_modalities = registry
                .map(|model| model.input_modalities.clone())
                .filter(|modalities| !modalities.is_empty())
                .unwrap_or_else(|| vec!["text".to_string()]);
            let context_window = registry.map_or(200_000, |model| model.context_window);
            let max_output_tokens = registry.map_or(32_000, |model| model.max_output_tokens);
            let tools = registry.is_some_and(|model| model.tools);
            let reasoning = registry.is_some_and(|model| model.reasoning);
            let price = registry.map_or((0.0, 0.0, 0.0, 0.0), |model| {
                (
                    model.input_price,
                    model.output_price,
                    model.cache_read_price,
                    model.cache_write_price,
                )
            });
            let mut entry = json!({
                "id": id,
                "available": available.contains(&id),
                "contextWindow": context_window,
                "maxOutputTokens": max_output_tokens,
                "inputModalities": input_modalities,
                "outputModalities": ["text"],
                "tools": tools,
                "reasoning": reasoning,
                "price": {
                    "input": price.0,
                    "output": price.1,
                    "cacheRead": price.2,
                    "cacheWrite": price.3,
                },
                "promotion": [],
            });
            // Which provider serves this id. Its absence is why a console
            // could list thousands of models and still not say which of
            // them any one provider or subscription covers: the client had
            // no field to group them by.
            //
            // `route` is sent only when it differs from the id. It matches
            // for all but a handful of a 6,700-model catalogue, and 276 kB
            // of repeating the id back is worth more than the symmetry.
            if let Some(model) = registry {
                entry["provider"] = json!(model.provider_id);
                if model.route_id != id {
                    entry["route"] = json!(model.route_id);
                }
            }
            if caller_known && available.contains(&id) {
                if let Some(perf) = perf_json(&id) {
                    entry["perf"] = perf;
                }
            }
            if let Some(reason) = unavailable_reasons.get(&id) {
                entry["unavailable_reason"] = json!(reason);
            }
            entry
        })
        .collect::<Vec<_>>();
    json!({
        "catalogRevision": catalog_revision,
        "version": "v1",
        "models": models,
        "degraded": degraded,
    })
}

pub(super) fn openai(view: CatalogView) -> Value {
    let CatalogView {
        model_ids,
        available,
        registry_metadata,
        unavailable_reasons,
        caller_known,
    } = view;
    let models = model_ids
        .into_iter()
        .map(|id| {
            let owner = registry_metadata
                .get(&id)
                .map(|model| model.provider_id.as_str())
                .unwrap_or("brama");
            let mut entry = json!({
                "id": id,
                "object": "model",
                "owned_by": owner,
            });
            // Whether this gateway can serve the id, for a caller whose
            // identity makes the answer knowable. `data` is the public
            // models.dev catalogue, several thousand ids wide, and almost none
            // of them have a credential behind them on any one installation.
            // A caller with no way to tell them apart picks a plausible name
            // and gets `dependency_unavailable` at dispatch -- which is how a
            // downstream product came to default to a model that had never
            // been servable here. The Jeden schema has carried this field all
            // along; the OpenAI-shaped view had nowhere to say it.
            if caller_known {
                entry["available"] = json!(available.contains(&id));
                if let Some(perf) = perf_json(&id) {
                    entry["perf"] = perf;
                }
            }
            // A declared alias that cannot serve says why, to every caller: the
            // reason is this gateway's configuration, not anything about who is
            // asking, and hiding it is what produced "not in the catalog".
            if let Some(reason) = unavailable_reasons.get(&id) {
                entry["available"] = json!(false);
                entry["unavailable_reason"] = json!(reason);
            }
            entry
        })
        .collect::<Vec<_>>();
    json!({
        "object": "list",
        "data": models,
    })
}
