//! The model limits this build carries for subscription providers, read
//! strictly and ranked below the provider's own listing.
//!
//! The table is a copy of oh-my-pi's model metadata embedded at compile time.
//! A provider changes a model's limits after a build is cut, so a value its
//! own listing states wins; the table fills only what the listing left
//! unsaid. A table that does not parse, or a row missing a field, is an error
//! naming it: read leniently, a broken table emptied itself and every model
//! moved, unannounced, to whatever discovery happened to say.

use std::collections::HashMap;
use std::sync::LazyLock;

use serde_json::Value;

use super::RegistryModel;

/// oh-my-pi model metadata, embedded at compile time.
static OMP_MODEL_METADATA: &str = include_str!("../../omp-model-metadata.json");

#[derive(Clone, Debug)]
struct OmpModelMetadata {
    context_window: u64,
    max_output_tokens: u64,
    reasoning: bool,
    input_modalities: Vec<String>,
}

type Table = HashMap<String, HashMap<String, OmpModelMetadata>>;

/// Parsed view of `OMP_MODEL_METADATA`: provider_id -> model_id -> metadata,
/// or the first thing in it that does not read.
static OMP_METADATA: LazyLock<Result<Table, String>> = LazyLock::new(|| {
    let document: Value = serde_json::from_str(OMP_MODEL_METADATA)
        .map_err(|error| format!("the embedded model table does not parse: {error}"))?;
    let Value::Object(entries) = document else {
        return Err("the embedded model table is not an object of providers".to_string());
    };
    let mut providers = HashMap::new();
    for (provider_id, models) in entries {
        let rows = models.as_array().ok_or_else(|| {
            format!("the embedded model table's {provider_id} entry is not a list of models")
        })?;
        let mut table = HashMap::new();
        for (position, row) in rows.iter().enumerate() {
            let (model_id, metadata) = omp_metadata_from_value(row).map_err(|missing| {
                format!(
                    "the embedded model table's {provider_id} row at position {position} {missing}"
                )
            })?;
            table.insert(model_id, metadata);
        }
        providers.insert(provider_id, table);
    }
    Ok(providers)
});

fn omp_metadata_from_value(row: &Value) -> Result<(String, OmpModelMetadata), String> {
    let text = |name: &str| row.get(name).and_then(Value::as_str);
    let count = |name: &str| row.get(name).and_then(Value::as_u64);
    let model_id = text("id").ok_or("has no text id")?;
    let context_window = count("contextWindow").ok_or("has no whole contextWindow")?;
    let max_output_tokens = count("maxTokens").ok_or("has no whole maxTokens")?;
    let reasoning = row
        .get("reasoning")
        .and_then(Value::as_bool)
        .ok_or("has no boolean reasoning")?;
    let input_modalities = row
        .get("input")
        .and_then(Value::as_array)
        .ok_or("has no input list")?
        .iter()
        .map(|value| value.as_str().map(str::to_string).ok_or("lists an input that is not text"))
        .collect::<Result<Vec<_>, _>>()?;
    if input_modalities.is_empty() {
        return Err(format!("({model_id}) lists no input modality"));
    }
    Ok((
        model_id.to_string(),
        OmpModelMetadata {
            context_window,
            max_output_tokens,
            reasoning,
            input_modalities,
        },
    ))
}

fn table() -> Result<&'static Table, String> {
    OMP_METADATA.as_ref().map_err(String::clone)
}

/// Fill what a provider's own listing left unsaid with this build's table,
/// on a `model_id` match. Only providers present in the table ("claude-code",
/// "codex", "kimi") are affected; a value the listing stated is kept, and
/// route/model ids, tools and prices are left untouched.
pub(in crate::providers::adapter) fn apply_omp_model_metadata(
    provider_id: &str,
    models: &mut [RegistryModel],
) -> Result<(), String> {
    let Some(table) = table()?.get(provider_id) else {
        return Ok(());
    };
    for model in models.iter_mut() {
        if let Some(metadata) = table.get(&model.model_id) {
            model.context_window = model.context_window.or(Some(metadata.context_window));
            model.max_output_tokens = model.max_output_tokens.or(Some(metadata.max_output_tokens));
            model.reasoning = model.reasoning.or(Some(metadata.reasoning));
            if model.input_modalities.is_empty() {
                model.input_modalities = metadata.input_modalities.clone();
            }
        }
    }
    Ok(())
}

/// The output limit this build's table states for `model_id` on
/// `provider_id`, if any.
pub(crate) fn known_max_output_tokens(
    provider_id: &str,
    model_id: &str,
) -> Result<Option<u64>, String> {
    Ok(table()?
        .get(provider_id)
        .and_then(|models| models.get(model_id))
        .map(|metadata| metadata.max_output_tokens))
}

/// The answer length a wire that requires one is sent with: the caller's,
/// else the limit this build's table states. A subscription route has
/// already sized the request from the provider's own listing before it gets
/// here; a request neither the caller nor any source sizes is refused here,
/// naming that, instead of by the provider.
pub(crate) fn required_output_limit(
    provider_id: &str,
    model_id: &str,
    requested: Option<u32>,
) -> Result<u64, String> {
    if let Some(requested) = requested {
        return Ok(u64::from(requested));
    }
    known_max_output_tokens(provider_id, model_id)?.ok_or_else(|| {
        format!(
            "{provider_id}/{model_id}: this wire requires max_tokens; the request set none and nothing states an output limit for the model, so set max_tokens"
        )
    })
}
