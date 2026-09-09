//! What the two single-shape endpoints accept. Both name exactly one model and
//! take either one string or a non-empty list of them; `deny_unknown_fields` is
//! what makes a mistyped option a refusal rather than a dropped intention.

use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, Serialize)]
#[serde(untagged)]
pub(in crate::core::server) enum TextInput {
    One(String),
    Many(Vec<String>),
}

impl TextInput {
    pub(super) fn is_valid(&self) -> bool {
        match self {
            Self::One(value) => !value.is_empty(),
            Self::Many(values) => {
                !values.is_empty() && values.iter().all(|value| !value.is_empty())
            }
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::core::server) struct EmbeddingRequest {
    pub(super) model: String,
    pub(super) input: TextInput,
    #[serde(default)]
    pub(super) encoding_format: Option<String>,
    #[serde(default)]
    pub(super) dimensions: Option<u32>,
    #[serde(default)]
    pub(super) user: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::core::server) struct ModerationRequest {
    pub(super) model: String,
    pub(super) input: TextInput,
}
