//! Part of `model_catalog`, split out to keep every file inside the line limit.
#![allow(unused_imports)]

use super::*;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};
use serde_json::Value;
use tokio::sync::{Mutex, RwLock};
use crate::providers::adapter::RegistryModel;

pub(crate) fn protocol_for(npm: &str) -> (CatalogProtocol, CatalogAuth) {
    if npm == "@ai-sdk/anthropic" {
        return (CatalogProtocol::AnthropicMessages, CatalogAuth::XApiKey);
    }
    if npm == "@ai-sdk/google" {
        return (
            CatalogProtocol::GoogleGenerateContent,
            CatalogAuth::GoogleApiKey,
        );
    }
    if npm == "@ai-sdk/openai-compatible"
        || matches!(
            npm,
            "@ai-sdk/openai"
                | "@ai-sdk/xai"
                | "@ai-sdk/mistral"
                | "@ai-sdk/cerebras"
                | "@ai-sdk/perplexity"
                | "@ai-sdk/deepinfra"
                | "@openrouter/ai-sdk-provider"
                | "@ai-sdk/togetherai"
                | "@ai-sdk/gateway"
                | "@ai-sdk/groq"
                | "venice-ai-sdk-provider"
                | "merge-gateway-ai-sdk-provider"
                | "ai-gateway-provider"
        )
    {
        return (CatalogProtocol::OpenAiChat, CatalogAuth::Bearer);
    }
    (CatalogProtocol::Unsupported, CatalogAuth::Bearer)
}

pub(crate) fn cost(model: &Value, key: &str) -> f64 {
    model
        .get("cost")
        .and_then(|cost| cost.get(key))
        .and_then(Value::as_f64)
        .unwrap_or(0.0)
}

pub(crate) fn valid_provider_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

pub(crate) fn valid_model_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 512
        && value.trim() == value
        && !value.chars().any(char::is_control)
}
