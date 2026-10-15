//! One row of a provider's own model listing, read into a registry model.

use serde_json::Value;

use super::facets::listed_output_modalities;
use super::{valid_model_id, ProviderDescriptor, RegistryModel};

pub(in crate::providers::adapter) fn model_from_value(
    descriptor: &ProviderDescriptor,
    row: &Value,
) -> Option<RegistryModel> {
    let id = row
        .get("id")
        .or_else(|| row.get("name"))
        .and_then(Value::as_str)?;
    if !valid_model_id(id) {
        return None;
    }
    let context_window = ["context_window", "context_length", "max_model_len"]
        .into_iter()
        .find_map(|key| row.get(key).and_then(Value::as_u64));
    let max_output_tokens = ["max_output_tokens", "max_tokens"]
        .into_iter()
        .find_map(|key| row.get(key).and_then(Value::as_u64));
    let display_name = match row.get("name").and_then(Value::as_str) {
        Some(name) => name,
        None => id,
    };
    Some(RegistryModel {
        route_id: format!("{}/{}", descriptor.id, id),
        provider_id: descriptor.id.to_string(),
        model_id: id.to_string(),
        display_name: display_name.to_string(),
        context_window,
        max_output_tokens,
        // A provider's own listing states what it serves, not what a model
        // takes, whether it calls tools, how the weights were published or
        // what it costs. Nothing here knows, so nothing here claims.
        input_modalities: Vec::new(),
        output_modalities: listed_output_modalities(row),
        open_weights: None,
        tools: None,
        reasoning: stated_reasoning(row),
        input_price: None,
        output_price: None,
        cache_read_price: None,
        cache_write_price: None,
    })
}

/// Whether a model reasons is what the listing states about it, not what its
/// name looks like: OpenRouter lists `reasoning` among a model's
/// `supported_parameters`, others carry `capabilities.reasoning`. A listing
/// that states neither says nothing.
fn stated_reasoning(row: &Value) -> Option<bool> {
    let capability = row
        .pointer("/capabilities/reasoning")
        .and_then(Value::as_bool);
    let parameter = row
        .get("supported_parameters")
        .and_then(Value::as_array)
        .map(|listed| listed.iter().any(|item| item.as_str() == Some("reasoning")));
    match (capability, parameter) {
        (Some(capability), Some(parameter)) => Some(capability || parameter),
        (Some(stated), None) | (None, Some(stated)) => Some(stated),
        (None, None) => None,
    }
}
