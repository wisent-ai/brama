//! Which headers a credential rides to the provider in.

use reqwest::RequestBuilder;

use super::super::super::registry::{AuthKind, ProviderDescriptor, WireProtocol};
use super::credential_account_id;
use crate::subscription_dispatch::model_catalog::{CatalogAuth, CatalogProvider};

fn authorize(
    builder: RequestBuilder,
    descriptor: &ProviderDescriptor,
    key: &str,
) -> RequestBuilder {
    match descriptor.auth {
        AuthKind::None => builder,
        AuthKind::Bearer => builder.bearer_auth(key),
        AuthKind::XApiKey => builder
            .header("x-api-key", key)
            .header("anthropic-version", "2023-06-01"),
        AuthKind::AnthropicBearer => builder
            .bearer_auth(key)
            .header("anthropic-version", "2023-06-01")
            .header("anthropic-beta", "oauth-2025-04-20"),
    }
}

/// `authorize` plus the extra headers the Codex ChatGPT-account backend
/// requires. A no-op for every other provider, so their requests stay
/// byte-identical.
pub(in crate::providers::adapter) fn authorize_provider(
    builder: RequestBuilder,
    descriptor: &ProviderDescriptor,
    key: &str,
    secret: &str,
) -> RequestBuilder {
    let builder = authorize(builder, descriptor, key);
    if descriptor.wire != WireProtocol::OpenAiResponses {
        return builder;
    }
    let builder = builder
        .header("OpenAI-Beta", "responses=experimental")
        .header("originator", "codex_cli_rs");
    match credential_account_id(secret) {
        Some(account_id) => builder.header("chatgpt-account-id", account_id),
        None => builder,
    }
}

pub(in crate::providers::adapter) fn authorize_catalog(
    builder: RequestBuilder,
    descriptor: &CatalogProvider,
    key: &str,
) -> RequestBuilder {
    match descriptor.auth {
        CatalogAuth::Bearer => builder.bearer_auth(key),
        CatalogAuth::XApiKey => builder
            .header("x-api-key", key)
            .header("anthropic-version", "2023-06-01"),
        CatalogAuth::GoogleApiKey => builder.header("x-goog-api-key", key),
    }
}
