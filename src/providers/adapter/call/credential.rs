//! Reducing a stored document to the string a provider will accept.

mod authorization;
mod document;

use serde_json::Value;

use super::super::registry::{AuthKind, ProviderDescriptor};
use document::{credential_document, credential_shape};

pub(in crate::providers::adapter) use authorization::{authorize_catalog, authorize_provider};

/// The document fields a provider credential is read from, named in the order
/// they are tried, for a message an operator can act on.
const SUPPORTED_KEY_FIELDS: &str = "key, apiKey, api_key, access, accessToken, \
     access_token, token, tokens.access_token, claudeAiOauth.accessToken";

/// Reduce a stored credential to the bearer a provider will accept.
///
/// `item` is the vault coordinate the secret was redeemed from, and it is in
/// the failure message because the repair is always at that coordinate.
///
/// Also the donation boundary's predicate: a document this cannot reduce is a
/// document no request could have presented, so banking it can only destroy the
/// credential already at that coordinate.
pub(crate) fn credential_key(item: &str, secret: &str) -> Result<String, String> {
    let trimmed = secret.trim();
    if trimmed.is_empty() {
        return Err(format!("Skarbiec item `{item}` holds an empty credential"));
    }
    let Some(document) = credential_document(trimmed) else {
        return Ok(trimmed.to_string());
    };
    // An envelope around a bare secret unwraps to a string, and so does a
    // credential that was stored as a quoted JSON string. Both are the key.
    if let Some(key) = document.as_str() {
        let key = key.trim();
        if key.is_empty() {
            return Err(format!("Skarbiec item `{item}` holds an empty credential"));
        }
        return Ok(key.to_string());
    }
    let candidates = [
        document.pointer("/key"),
        document.pointer("/apiKey"),
        document.pointer("/api_key"),
        document.pointer("/access"),
        document.pointer("/accessToken"),
        document.pointer("/access_token"),
        document.pointer("/token"),
        document.pointer("/tokens/access_token"),
        document.pointer("/claudeAiOauth/accessToken"),
    ];
    let key = candidates
        .into_iter()
        .flatten()
        .find_map(Value::as_str)
        .map(str::to_string)
        .filter(|value| !value.trim().is_empty());
    key.ok_or_else(|| {
        format!(
            "Skarbiec item `{item}` holds {}, which carries no credential: expected a bare \
             secret, a typed envelope carrying it under `value`, or an object carrying it \
             under one of {SUPPORTED_KEY_FIELDS}",
            credential_shape(&document)
        )
    })
}

pub(in crate::providers::adapter) fn provider_credential_key(
    descriptor: &ProviderDescriptor,
    item: &str,
    secret: &str,
) -> Result<String, String> {
    if descriptor.auth == AuthKind::None {
        return Ok(String::new());
    }
    credential_key(item, secret)
}

fn credential_account_id(secret: &str) -> Option<String> {
    credential_document(secret)?
        .pointer("/tokens/account_id")
        .and_then(Value::as_str)
        .map(str::to_string)
        .filter(|value| !value.trim().is_empty())
}
