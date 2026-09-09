//! What a provider is, and which providers this build knows.
//!
//! A provider declaration is the whole of what this gateway assumes about one
//! vendor before any call is made: where it lives, which wire it speaks, how a
//! credential rides on its requests, and which models it is known to serve even
//! when its own listing is unavailable.

mod address;
mod advertised_model;
mod known_limits;
mod roster;
mod route;

pub(in crate::providers::adapter) use address::{
    endpoint, provider_base_url, provider_base_url_for, provider_base_url_override,
    validated_provider_base_url,
};
pub(in crate::providers::adapter) use advertised_model::model_from_value;
pub(in crate::providers::adapter) use known_limits::apply_omp_model_metadata;
pub(in crate::providers::adapter) use route::{valid_model_id, valid_provider_id};
pub use route::{
    provider_id_from_route, route, supports_chat_route, supports_embedding_route,
    supports_moderation_route,
};

use roster::PROVIDERS;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WireProtocol {
    OpenAiChat,
    AnthropicMessages,
    OpenAiResponses,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthKind {
    None,
    Bearer,
    XApiKey,
    AnthropicBearer,
}

#[derive(Clone, Copy, Debug)]
pub struct ProviderDescriptor {
    pub id: &'static str,
    pub display_name: &'static str,
    pub base_url: &'static str,
    pub models_path: &'static str,
    pub chat_path: &'static str,
    pub wire: WireProtocol,
    pub auth: AuthKind,
    pub static_models: &'static [&'static str],
}

#[derive(Clone, Debug)]
pub struct RegistryModel {
    pub route_id: String,
    pub provider_id: String,
    pub model_id: String,
    pub context_window: u64,
    pub max_output_tokens: u64,
    pub input_modalities: Vec<String>,
    pub tools: bool,
    pub reasoning: bool,
    pub input_price: f64,
    pub output_price: f64,
    pub cache_read_price: f64,
    pub cache_write_price: f64,
}

pub fn providers() -> &'static [ProviderDescriptor] {
    PROVIDERS
}

pub fn provider(id: &str) -> Option<&'static ProviderDescriptor> {
    PROVIDERS.iter().find(|provider| provider.id == id)
}

pub(crate) fn provider_requires_credential(provider_id: &str) -> bool {
    provider(provider_id).is_none_or(|descriptor| descriptor.auth != AuthKind::None)
}
