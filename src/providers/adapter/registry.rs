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
pub use route::{
    native_decision_route, provider_id_from_route, route, supports_chat_route,
    supports_decision_route, supports_embedding_route, supports_moderation_route,
};
pub(in crate::providers::adapter) use route::{valid_model_id, valid_provider_id};

use roster::PROVIDERS;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WireProtocol {
    OpenAiChat,
    AnthropicMessages,
    OpenAiResponses,
    /// TypeSafe AI's System One wire: a state and typed questions in, typed
    /// answers out. It carries no messages and returns no text, so nothing on
    /// the chat path may reach it.
    TypeSafeSystemOne,
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
    /// The generation path, empty when this provider serves no chat.
    pub chat_path: &'static str,
    /// The typed-decision path, empty when this provider serves no decisions.
    pub decision_path: &'static str,
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

/// Where this provider's traffic is allowed to go, as the request path will
/// resolve it: the deployment's override when it set one, else the declared
/// origin, both checked against the trusted-host policy.
///
/// `Ok(None)` is the third answer and it belongs to one provider: the local
/// model server's endpoint is declared per deployment in the route registry,
/// one per model, so there is no single origin to state here.
///
/// A declaration nothing confronts with that policy is a provider that
/// cannot serve and says nothing about it until a caller is refused, which
/// is why the console reads this and a test asserts every declared provider
/// answers it.
pub fn provider_endpoint(provider_id: &str) -> Result<Option<String>, String> {
    let descriptor =
        provider(provider_id).ok_or_else(|| format!("provider `{provider_id}` is not declared"))?;
    if address::provider_base_url_override(descriptor.id).is_none()
        && descriptor.id == LOCAL_MODEL_SERVER
    {
        return Ok(None);
    }
    address::provider_base_url(descriptor).map(Some)
}

/// The one provider whose endpoint each deployment declares for itself, in
/// the route registry beside the deployment name a route points at.
const LOCAL_MODEL_SERVER: &str = "local-openai";

pub(crate) fn provider_requires_credential(provider_id: &str) -> bool {
    provider(provider_id).is_none_or(|descriptor| descriptor.auth != AuthKind::None)
}
