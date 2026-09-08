//! What a provider in the public catalog is, and how Brama would speak to it.
//!
//! models.dev describes a provider by the npm package its own SDK ships, which
//! is the only field in that document that says anything about the wire. This
//! module is the one place that reading happens, so adding a provider Brama can
//! execute is one arm of one match.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CatalogProtocol {
    OpenAiChat,
    AnthropicMessages,
    GoogleGenerateContent,
    Unsupported,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CatalogAuth {
    Bearer,
    XApiKey,
    GoogleApiKey,
}

#[derive(Clone, Debug)]
pub struct CatalogProvider {
    pub id: String,
    pub display_name: String,
    pub protocol: CatalogProtocol,
    pub auth: CatalogAuth,
}

impl CatalogProvider {
    pub fn executable(&self) -> bool {
        self.protocol != CatalogProtocol::Unsupported
    }
}

/// The wire and the credential header a provider's own SDK package implies.
///
/// A package nobody here has mapped is `Unsupported`, which keeps the provider
/// in the catalog as a name while refusing to route to it: the catalog is
/// public metadata, and executing against it is a decision this fleet makes.
pub(super) fn protocol_for(npm: &str) -> (CatalogProtocol, CatalogAuth) {
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
