//! What a `provider/model` string names, and what that route is good for.

use std::borrow::Cow;

use super::{provider, ProviderDescriptor};

const QWEN_DEFAULT_MODEL: &str = "qwen-max";
const OPENAI_DEFAULT_MODEL: &str = "gpt-5.4";
const OPENAI_EMBEDDING_MODEL: &str = "text-embedding-3-small";
const OPENAI_MODERATION_MODEL: &str = "omni-moderation-latest";

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
    route(value).is_some_and(|(_, model_id)| {
        model_id.as_ref() != OPENAI_EMBEDDING_MODEL && model_id.as_ref() != OPENAI_MODERATION_MODEL
    })
}

pub fn supports_embedding_route(value: &str) -> bool {
    value == "openai/embeddings" && route(value).is_some()
}

pub fn supports_moderation_route(value: &str) -> bool {
    value == "openai/moderation" && route(value).is_some()
}

pub(in crate::providers::adapter) fn valid_model_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 512
        && value.trim() == value
        && !value.chars().any(char::is_control)
}

pub(in crate::providers::adapter) fn valid_provider_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}
