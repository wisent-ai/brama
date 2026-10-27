//! Turning one models.dev document into the models this fleet can name.
//!
//! Every row is read defensively, because the document is public metadata from
//! another project: a provider or model whose identifier could not be used in a
//! route is skipped rather than carried, and a fact a row does not state (a
//! price, tool use, reasoning, a modality) is carried as unstated, never as a
//! zero, a `false` or text.

use std::collections::HashMap;

use serde_json::Value;

use crate::providers::adapter::{valid_model_id, valid_provider_id, RegistryModel};

use super::provider::{protocol_for, CatalogAuth, CatalogProtocol, CatalogProvider};
use super::CatalogSnapshot;

/// A row's identifier: models.dev keys every provider and model by its id,
/// and a row that also states `id` is read by that.
fn row_id<'a>(row: &'a Value, key: &'a str) -> &'a str {
    match row.get("id").and_then(Value::as_str) {
        Some(id) => id.trim(),
        None => key.trim(),
    }
}

/// What a row calls itself, or its id when it states no name.
fn row_name(row: &Value, id: &str) -> String {
    match row.get("name").and_then(Value::as_str) {
        Some(name) => name.to_string(),
        None => id.to_string(),
    }
}

pub(super) fn parse_catalog(raw: &str) -> Result<CatalogSnapshot, String> {
    let root: Value = serde_json::from_str(raw)
        .map_err(|error| format!("the models.dev catalog is not JSON: {error}"))?;
    let rows = root
        .as_object()
        .ok_or_else(|| "models.dev root must be an object".to_string())?;
    let mut providers = HashMap::with_capacity(rows.len());
    let mut models = Vec::new();
    let mut latest_update = String::new();

    for (catalog_key, row) in rows {
        let id = row_id(row, catalog_key);
        if !valid_provider_id(id) {
            continue;
        }
        // The SDK package a provider names decides its wire; a provider that
        // names none has no wire this gateway can speak.
        let (protocol, auth) = match row.get("npm").and_then(Value::as_str) {
            Some(npm) => protocol_for(npm),
            None => (CatalogProtocol::Unsupported, CatalogAuth::Bearer),
        };
        providers.insert(
            id.to_string(),
            CatalogProvider {
                id: id.to_string(),
                display_name: row_name(row, id),
                protocol,
                auth,
            },
        );

        let Some(model_rows) = row.get("models").and_then(Value::as_object) else {
            continue;
        };
        models.reserve(model_rows.len());
        for (model_key, model) in model_rows {
            let model_id = row_id(model, model_key);
            if !valid_model_id(model_id) {
                continue;
            }
            if let Some(updated) = model.get("last_updated").and_then(Value::as_str) {
                if updated > latest_update.as_str() {
                    latest_update = updated.to_string();
                }
            }
            models.push(RegistryModel {
                route_id: format!("{id}/{model_id}"),
                provider_id: id.to_string(),
                model_id: model_id.to_string(),
                display_name: row_name(model, model_id),
                context_window: model.pointer("/limit/context").and_then(Value::as_u64),
                max_output_tokens: model.pointer("/limit/output").and_then(Value::as_u64),
                input_modalities: modalities(model, "input"),
                output_modalities: modalities(model, "output"),
                // models.dev states these per model. A row that omits one
                // says nothing, and `None` is that answer rather than a claim
                // of proprietary weights, of no tools or of no reasoning.
                open_weights: model.get("open_weights").and_then(Value::as_bool),
                tools: model.get("tool_call").and_then(Value::as_bool),
                reasoning: model.get("reasoning").and_then(Value::as_bool),
                input_price: cost(model, "input"),
                output_price: cost(model, "output"),
                cache_read_price: cost(model, "cache_read"),
                cache_write_price: cost(model, "cache_write"),
            });
        }
    }

    models.sort_by(|left, right| left.route_id.cmp(&right.route_id));
    models.dedup_by(|left, right| left.route_id == right.route_id);
    let revision = format!(
        "models-dev-{latest_update}-{}-{}",
        providers.len(),
        models.len()
    );
    Ok(CatalogSnapshot {
        providers,
        models,
        revision,
    })
}

/// One side of a row's `modalities` block, empty when the row states
/// nothing. Both sides are read the same way, so an image model that states
/// its output cannot end up described by whatever the input side happened to
/// carry.
fn modalities(model: &Value, side: &str) -> Vec<String> {
    match model
        .pointer(&format!("/modalities/{side}"))
        .and_then(Value::as_array)
    {
        Some(values) => values
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect(),
        None => Vec::new(),
    }
}

/// The price models.dev publishes for one token kind, or `None` when it
/// publishes none: an unpublished price is not a price of zero.
fn cost(model: &Value, key: &str) -> Option<f64> {
    model
        .get("cost")
        .and_then(|cost| cost.get(key))
        .and_then(Value::as_f64)
}
