//! Which headers a credential rides to the provider in.

use reqwest::RequestBuilder;

use super::super::super::registry::{AuthKind, ProviderDescriptor, WireProtocol};
use super::credential_account_id;
use crate::subscription_dispatch::model_catalog::{CatalogAuth, CatalogProvider};
/// Anthropic Messages API version: https://platform.claude.com/docs/en/api/versioning
const ANTHROPIC_API_VERSION: &str = "2023-06-01";

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
            .header("anthropic-version", ANTHROPIC_API_VERSION),
        AuthKind::AnthropicBearer => super::claude_code::headers(
            builder
                .bearer_auth(key)
                .header("anthropic-version", ANTHROPIC_API_VERSION),
        ),
        AuthKind::XiApiKey => builder.header("xi-api-key", key),
        AuthKind::GoogleApiKey => builder.header("x-goog-api-key", key),
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
) -> Result<RequestBuilder, String> {
    let account_id = if descriptor.wire == WireProtocol::OpenAiResponses {
        credential_account_id(secret)?
    } else {
        None
    };
    Ok(authorize_with_account(
        builder,
        descriptor,
        key,
        account_id.as_deref(),
    ))
}

/// Apply headers after the account claim has already been checked. Media
/// adapters validate once when constructing a call with several operations.
pub(in crate::providers::adapter) fn authorize_with_account(
    builder: RequestBuilder,
    descriptor: &ProviderDescriptor,
    key: &str,
    account_id: Option<&str>,
) -> RequestBuilder {
    let builder = authorize(builder, descriptor, key);
    if descriptor.wire != WireProtocol::OpenAiResponses {
        return builder;
    }
    let builder = builder
        .header("OpenAI-Beta", "responses=experimental")
        .header("originator", "codex_cli_rs");
    match account_id {
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
            .header("anthropic-version", ANTHROPIC_API_VERSION),
        CatalogAuth::GoogleApiKey => builder.header("x-goog-api-key", key),
    }
}
