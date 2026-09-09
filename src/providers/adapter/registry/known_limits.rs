//! The model limits this build already knows, whatever a listing says.

use std::collections::HashMap;
use std::sync::LazyLock;

use serde_json::Value;

use super::RegistryModel;

/// oh-my-pi model metadata extracted from the local models.db, embedded at
/// compile time so subscription providers get authoritative limits.
static OMP_MODEL_METADATA: &str = include_str!("../../omp-model-metadata.json");

#[derive(Clone, Debug)]
struct OmpModelMetadata {
    context_window: u64,
    max_output_tokens: u64,
    reasoning: bool,
    input_modalities: Vec<String>,
}

/// Parsed view of `OMP_MODEL_METADATA`: provider_id -> model_id -> metadata.
/// Malformed entries are skipped instead of failing the whole table.
static OMP_METADATA: LazyLock<HashMap<String, HashMap<String, OmpModelMetadata>>> =
    LazyLock::new(|| {
        let mut providers = HashMap::new();
        let Ok(Value::Object(entries)) = serde_json::from_str::<Value>(OMP_MODEL_METADATA) else {
            return providers;
        };
        for (provider_id, models) in entries {
            let Some(rows) = models.as_array() else {
                continue;
            };
            let mut table = HashMap::new();
            for row in rows {
                if let Some((model_id, metadata)) = omp_metadata_from_value(row) {
                    table.insert(model_id, metadata);
                }
            }
            providers.insert(provider_id, table);
        }
        providers
    });

fn omp_metadata_from_value(row: &Value) -> Option<(String, OmpModelMetadata)> {
    let model_id = row.get("id")?.as_str()?;
    let context_window = row.get("contextWindow")?.as_u64()?;
    let max_output_tokens = row.get("maxTokens")?.as_u64()?;
    let reasoning = row.get("reasoning")?.as_bool()?;
    let input_modalities = row
        .get("input")?
        .as_array()?
        .iter()
        .filter_map(|value| value.as_str().map(str::to_string))
        .collect::<Vec<_>>();
    if input_modalities.is_empty() {
        return None;
    }
    Some((
        model_id.to_string(),
        OmpModelMetadata {
            context_window,
            max_output_tokens,
            reasoning,
            input_modalities,
        },
    ))
}

/// Override discovered models with authoritative oh-my-pi metadata on a
/// `model_id` match. Only providers present in the embedded table
/// ("claude-code", "codex", "kimi") are affected; route/model ids, tools and
/// prices are left untouched.
pub(in crate::providers::adapter) fn apply_omp_model_metadata(
    provider_id: &str,
    models: &mut [RegistryModel],
) {
    let Some(table) = OMP_METADATA.get(provider_id) else {
        return;
    };
    for model in models.iter_mut() {
        if let Some(metadata) = table.get(&model.model_id) {
            model.context_window = metadata.context_window;
            model.max_output_tokens = metadata.max_output_tokens;
            model.reasoning = metadata.reasoning;
            model.input_modalities = metadata.input_modalities.clone();
        }
    }
}
