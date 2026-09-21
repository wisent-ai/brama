//! What a `provider/model` string names, and what that route is good for.

use std::borrow::Cow;

use super::{provider, ProviderDescriptor};

const QWEN_DEFAULT_MODEL: &str = "qwen-max";
const OPENAI_DEFAULT_MODEL: &str = "gpt-5.4";
const OPENAI_EMBEDDING_MODEL: &str = "text-embedding-3-small";
const OPENAI_MODERATION_MODEL: &str = "omni-moderation-latest";
/// A model id is at most 512 bytes; a provider id at most 128.
const MAX_MODEL_ID_BYTES: usize = 512;
const MAX_PROVIDER_ID_BYTES: usize = 128;

pub fn provider_id_from_route(value: &str) -> Option<&str> {
    let (provider_id, model_id) = value.split_once('/')?;
    (valid_provider_id(provider_id) && valid_model_id(model_id)).then_some(provider_id)
}

pub fn route(value: &str) -> Option<(&'static ProviderDescriptor, Cow<'_, str>)> {
    let (provider_id, model_id) = value.split_once('/')?;
    let descriptor = provider(provider_id)?;
    if !valid_model_id(model_id) {
        return None;
    }
    let concrete = match value {
        "qwen/default" => QWEN_DEFAULT_MODEL,
        "openai/default" => OPENAI_DEFAULT_MODEL,
        "openai/embeddings" => OPENAI_EMBEDDING_MODEL,
        "openai/moderation" => OPENAI_MODERATION_MODEL,
        _ => return Some((descriptor, Cow::Borrowed(model_id))),
    };
    Some((descriptor, Cow::Borrowed(concrete)))
}

pub fn supports_chat_route(value: &str) -> bool {
    route(value).is_some_and(|(descriptor, model_id)| {
        !descriptor.chat_path.is_empty()
            && model_id.as_ref() != OPENAI_EMBEDDING_MODEL
            && model_id.as_ref() != OPENAI_MODERATION_MODEL
    })
}

/// Whether this route is served by a provider that answers typed decisions in
/// its own wire, rather than by rendering them onto a chat model.
pub fn native_decision_route(value: &str) -> bool {
    route(value).is_some_and(|(descriptor, _)| !descriptor.decision_path.is_empty())
}

/// Whether `POST /v1/decisions` can be served over this route at all: either
/// the provider answers the decision wire itself, or it serves chat and Brama
/// renders the questions onto it.
pub fn supports_decision_route(value: &str) -> bool {
    native_decision_route(value) || supports_chat_route(value)
}

pub fn supports_embedding_route(value: &str) -> bool {
    value == "openai/embeddings" && route(value).is_some()
}

pub fn supports_moderation_route(value: &str) -> bool {
    value == "openai/moderation" && route(value).is_some()
}

pub(in crate::providers::adapter) fn valid_model_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_MODEL_ID_BYTES
        && value.trim() == value
        && !value.chars().any(char::is_control)
}

pub(in crate::providers::adapter) fn valid_provider_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_PROVIDER_ID_BYTES
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}
