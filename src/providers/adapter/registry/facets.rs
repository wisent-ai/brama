//! What a model is, beyond the route that reaches it.
//!
//! Two questions a caller asks before it picks a name, and neither was
//! answerable here until now: what does this model produce, and are its
//! weights published. Each is decided from one source and nothing else — the
//! output modalities the catalogue states and the `open_weights` flag it
//! carries — so a facet is never a guess dressed as a fact. The third
//! question, which category a model belongs to, is not decided here at all:
//! categories are declared by the operator in the route registry
//! (`crate::core::inference_routes::categories`), because no public catalogue
//! carries them and a gateway that invented them would be publishing an
//! opinion as metadata.

/// What one model produces. Decided from the output modalities its source
/// declares, never from the prose around them: a text-to-video model whose
/// description reads "Image model for prompt-driven generation" is a video
/// model, because `modalities.output` says `["video"]`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModelKind {
    Text,
    Image,
    Video,
    Audio,
}

impl ModelKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ModelKind::Text => "text",
            ModelKind::Image => "image",
            ModelKind::Video => "video",
            ModelKind::Audio => "audio",
        }
    }

    /// The name a caller writes in `?kind=`, or nothing when it wrote
    /// something this gateway has no kind for.
    pub fn parse(value: &str) -> Option<ModelKind> {
        match value.trim().to_ascii_lowercase().as_str() {
            "text" => Some(ModelKind::Text),
            "image" => Some(ModelKind::Image),
            "video" => Some(ModelKind::Video),
            "audio" => Some(ModelKind::Audio),
            _ => None,
        }
    }
}

/// A model that emits video is a video model even when it also emits text, and
/// the same precedence runs down to audio: the heaviest artifact decides,
/// because that is the endpoint shape the caller has to post to.
pub fn kind_from_output(output_modalities: &[String]) -> ModelKind {
    let has = |wanted: &str| {
        output_modalities
            .iter()
            .any(|modality| modality.eq_ignore_ascii_case(wanted))
    };
    if has("video") {
        ModelKind::Video
    } else if has("image") {
        ModelKind::Image
    } else if has("audio") {
        ModelKind::Audio
    } else {
        ModelKind::Text
    }
}

/// The output modalities a provider's own model listing states, in the shapes
/// the listings this gateway reads actually use.
///
/// OpenRouter answers `architecture.output_modalities`, a models.dev-shaped
/// mirror answers `modalities.output`, and a listing may answer a flat
/// `output_modalities`. A listing that states none leaves the vector empty and
/// the reader falls back to text, which is what a chat listing is.
pub fn listed_output_modalities(row: &serde_json::Value) -> Vec<String> {
    let stated = [
        row.pointer("/architecture/output_modalities"),
        row.pointer("/modalities/output"),
        row.get("output_modalities"),
    ];
    for array in stated.into_iter().flatten() {
        let Some(values) = array.as_array() else {
            continue;
        };
        let modalities = values
            .iter()
            .filter_map(serde_json::Value::as_str)
            .map(str::to_string)
            .collect::<Vec<_>>();
        if !modalities.is_empty() {
            return modalities;
        }
    }
    Vec::new()
}
