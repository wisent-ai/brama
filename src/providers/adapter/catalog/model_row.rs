//! What one row of the model catalog listing means.

use serde_json::Value;

use super::super::registry::{valid_model_id, RegistryModel};

pub(in crate::providers::adapter) fn catalog_model_from_value(
    provider_id: &str,
    row: &Value,
) -> Option<RegistryModel> {
    let id = row
        .get("id")
        .or_else(|| row.get("name"))
        .and_then(Value::as_str)?
        .strip_prefix("models/")
        .unwrap_or_else(|| {
            row.get("id")
                .or_else(|| row.get("name"))
                .and_then(Value::as_str)
                .unwrap_or_default()
        });
    if !valid_model_id(id) {
        return None;
    }
    Some(RegistryModel {
        route_id: format!("{provider_id}/{id}"),
        provider_id: provider_id.to_string(),
        model_id: id.to_string(),
        context_window: row
            .get("inputTokenLimit")
            .or_else(|| row.get("context_window"))
            .or_else(|| row.get("context_length"))
            .and_then(Value::as_u64)
            .unwrap_or(128_000),
        max_output_tokens: row
            .get("outputTokenLimit")
            .or_else(|| row.get("max_output_tokens"))
            .or_else(|| row.get("max_tokens"))
            .and_then(Value::as_u64)
            .unwrap_or(16_384),
        input_modalities: vec!["text".into()],
        tools: true,
        reasoning: false,
        input_price: 0.0,
        output_price: 0.0,
        cache_read_price: 0.0,
        cache_write_price: 0.0,
    })
}
