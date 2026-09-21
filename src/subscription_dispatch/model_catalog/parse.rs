//! Turning one models.dev document into the models this fleet can name.
//!
//! Every row is read defensively, because the document is public metadata from
//! another project: a provider or model whose identifier could not be used in a
//! route is skipped rather than carried, and the numbers a route needs carry
//! the catalog's own conservative shape when a row omits them.

use std::collections::HashMap;

use serde_json::Value;

use crate::providers::adapter::RegistryModel;

use super::provider::{protocol_for, CatalogProvider};
use super::CatalogSnapshot;

/// A provider id is at most 128 bytes and a model id at most 512, the same bounds a
/// `provider/model` route is held to.
const MAX_PROVIDER_ID_BYTES: usize = 128;
const MAX_MODEL_ID_BYTES: usize = 512;

pub(super) fn parse_catalog(raw: &str) -> Result<CatalogSnapshot, String> {
    let root: Value = serde_json::from_str(raw).map_err(|error| error.to_string())?;
    let rows = root
        .as_object()
        .ok_or_else(|| "models.dev root must be an object".to_string())?;
    let mut providers = HashMap::with_capacity(rows.len());
    let mut models = Vec::new();
    let mut latest_update = String::new();

    for (catalog_key, row) in rows {
        let id = row
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or(catalog_key)
            .trim();
        if !valid_provider_id(id) {
            continue;
        }
        let npm = row.get("npm").and_then(Value::as_str).unwrap_or_default();
        let (protocol, auth) = protocol_for(npm);
        providers.insert(
            id.to_string(),
            CatalogProvider {
                id: id.to_string(),
                display_name: row
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or(id)
                    .to_string(),
                protocol,
                auth,
            },
        );

        let Some(model_rows) = row.get("models").and_then(Value::as_object) else {
            continue;
        };
        models.reserve(model_rows.len());
        for (model_key, model) in model_rows {
            let model_id = model
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or(model_key)
                .trim();
            if !valid_model_id(model_id) {
                continue;
            }
            if let Some(updated) = model.get("last_updated").and_then(Value::as_str) {
                if updated > latest_update.as_str() {
                    latest_update = updated.to_string();
                }
            }
            let input_modalities = modalities(model, "input", &["text".to_string()]);
            let output_modalities = modalities(model, "output", &["text".to_string()]);
            models.push(RegistryModel {
                route_id: format!("{id}/{model_id}"),
                provider_id: id.to_string(),
                model_id: model_id.to_string(),
                display_name: model
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or(model_id)
                    .to_string(),
                context_window: model
                    .pointer("/limit/context")
                    .and_then(Value::as_u64)
                    .unwrap_or(128_000),
                max_output_tokens: model
                    .pointer("/limit/output")
                    .and_then(Value::as_u64)
                    .unwrap_or(16_384),
                input_modalities,
                output_modalities,
                // models.dev states this per model and states it for every
                // model it carries. A row that omits it says nothing, and
                // `None` is that answer rather than a claim of proprietary
                // weights.
                open_weights: model.get("open_weights").and_then(Value::as_bool),
                tools: model
                    .get("tool_call")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                reasoning: model
                    .get("reasoning")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
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

/// One side of a row's `modalities` block, or the fallback when the row says
/// nothing. Both sides are read the same way, so an image model that states
/// its output cannot end up described by whatever the input side happened to
/// carry.
fn modalities(model: &Value, side: &str, fallback: &[String]) -> Vec<String> {
    model
        .pointer(&format!("/modalities/{side}"))
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .filter(|values| !values.is_empty())
        .unwrap_or_else(|| fallback.to_vec())
}

fn cost(model: &Value, key: &str) -> f64 {
    model
        .get("cost")
        .and_then(|cost| cost.get(key))
        .and_then(Value::as_f64)
        .unwrap_or(0.0)
}

/// A provider identifier has to survive being one half of a `provider/model`
/// route, so anything that could split or hide a route is refused here.
fn valid_provider_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_PROVIDER_ID_BYTES
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

fn valid_model_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_MODEL_ID_BYTES
        && value.trim() == value
        && !value.chars().any(char::is_control)
}
