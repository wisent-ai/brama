//! What one row of the model catalog listing means.

use serde_json::Value;

use super::super::registry::facets::listed_output_modalities;
use super::super::registry::{valid_model_id, RegistryModel};

/// A model the listing names, as it names it. Google's listing prefixes each
/// name with `models/`, which is the resource collection, not part of the id
/// a request sends.
pub(in crate::providers::adapter) fn catalog_model_from_value(
    provider_id: &str,
    row: &Value,
) -> Option<RegistryModel> {
    let listed = row
        .get("id")
        .or_else(|| row.get("name"))
        .and_then(Value::as_str)?;
    let id = match listed.strip_prefix("models/") {
        Some(id) => id,
        None => listed,
    };
    if !valid_model_id(id) {
        return None;
    }
    let display_name = match row
        .get("displayName")
        .or_else(|| row.get("name"))
        .and_then(Value::as_str)
    {
        Some(name) => name,
        None => id,
    };
    Some(RegistryModel {
        route_id: format!("{provider_id}/{id}"),
        provider_id: provider_id.to_string(),
        model_id: id.to_string(),
        display_name: display_name.to_string(),
        context_window: row
            .get("inputTokenLimit")
            .or_else(|| row.get("context_window"))
            .or_else(|| row.get("context_length"))
            .and_then(Value::as_u64),
        max_output_tokens: row
            .get("outputTokenLimit")
            .or_else(|| row.get("max_output_tokens"))
            .or_else(|| row.get("max_tokens"))
            .and_then(Value::as_u64),
        // The listing states neither what a model takes nor whether it calls
        // tools, reasons or costs anything, so none of it is claimed.
        input_modalities: Vec::new(),
        output_modalities: listed_output_modalities(row),
        open_weights: None,
        tools: None,
        reasoning: None,
        input_price: None,
        output_price: None,
        cache_read_price: None,
        cache_write_price: None,
    })
}
