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
        .find_map(|key| row.get(key).and_then(Value::as_u64))
        .unwrap_or(128_000);
    let max_output_tokens = ["max_output_tokens", "max_tokens"]
        .into_iter()
        .find_map(|key| row.get(key).and_then(Value::as_u64))
        .unwrap_or(16_384);
    let lower = id.to_ascii_lowercase();
    Some(RegistryModel {
        route_id: format!("{}/{}", descriptor.id, id),
        provider_id: descriptor.id.to_string(),
        model_id: id.to_string(),
        display_name: row
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or(id)
            .to_string(),
        context_window,
        max_output_tokens,
        input_modalities: vec!["text".into()],
        output_modalities: listed_output_modalities(row),
        // A provider's own listing states what it serves, not how the weights
        // were published. Nothing here knows, so nothing here claims.
        open_weights: None,
        tools: true,
        reasoning: lower.contains("reason")
            || lower.contains("thinking")
            || lower.contains("deepseek-r1")
            || lower.contains("o1")
            || lower.contains("o3")
            || lower.contains("o4"),
        input_price: 0.0,
        output_price: 0.0,
        cache_read_price: 0.0,
        cache_write_price: 0.0,
    })
}
