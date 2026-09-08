//! Which providers Brama can refresh, and what their credential looks like.
//!
//! Everything provider-specific lives here: the token endpoint and client id,
//! whether the exchange is JSON or form-encoded, where in the stored blob the
//! access and refresh tokens sit, and how a fresh grant is written back. A
//! new provider is added by extending the three matches below and nothing
//! else.

use serde_json::{json, Value};
use zeroize::Zeroize;
use zeroize::Zeroizing;

#[derive(Clone, Copy)]
pub(super) enum OAuthWire {
    Json,
    Form,
}

pub(super) struct OAuthProvider {
    pub(super) token_endpoint: &'static str,
    pub(super) client_id: &'static str,
    pub(super) wire: OAuthWire,
}

pub(super) fn oauth_provider(provider: &str) -> Option<OAuthProvider> {
    match provider {
        "claude-code" => Some(OAuthProvider {
            token_endpoint: "https://claude.ai/v1/oauth/token",
            client_id: "9d1c250a-e61b-44d9-88ed-5944d1962f5e",
            wire: OAuthWire::Json,
        }),
        "codex" => Some(OAuthProvider {
            token_endpoint: "https://auth.openai.com/oauth/token",
            client_id: "app_EMoamEEZ73f0CkXaXp7hrann",
            wire: OAuthWire::Form,
        }),
        "kimi" => Some(OAuthProvider {
            token_endpoint: "https://auth.kimi.com/api/oauth/token",
            client_id: "17e5f671-d194-4dfb-9706-5516cb48c098",
            wire: OAuthWire::Form,
        }),
        _ => None,
    }
}

/// Whether this provider's credentials are OAuth grants Brama can refresh at
/// all.
///
/// A caller that sweeps every subscription asks this before reading anything:
/// an API-key subscription has no access token that expires, so redeeming its
/// credential to discover that costs a vault read and learns nothing.
pub(in crate::gateway) fn supports_refresh(provider: &str) -> bool {
    oauth_provider(provider).is_some()
}

pub(super) fn access_token<'a>(blob: &'a Value, provider: &str) -> Option<&'a str> {
    let value = match provider {
        "claude-code" => blob.get("claudeAiOauth")?.get("accessToken")?,
        "codex" => blob.get("tokens")?.get("access_token")?,
        "kimi" => blob.get("access_token")?,
        _ => return None,
    };
    value.as_str().filter(|token| !token.is_empty())
}

pub(super) fn oauth_refresh_token(blob: &Value, provider: &str) -> Option<Zeroizing<String>> {
    let value = match provider {
        "claude-code" => blob.get("claudeAiOauth")?.get("refreshToken")?,
        "codex" => blob.get("tokens")?.get("refresh_token")?,
        "kimi" => blob.get("refresh_token")?,
        _ => return None,
    };
    value
        .as_str()
        .filter(|token| !token.is_empty())
        .map(|token| Zeroizing::new(token.to_owned()))
}

fn millis_per_second() -> i64 {
    "1000".parse().expect("valid milliseconds per second")
}

/// Write a fresh grant back into the stored blob, in that provider's own
/// shape. `false` means the blob was not the shape this provider stores, so
/// nothing was written and the caller must not persist it.
pub(super) fn patch_oauth_blob(
    blob: &mut Value,
    provider: &str,
    grant: &super::RefreshGrant,
    now: i64,
) -> bool {
    match provider {
        "claude-code" => {
            let Some(oauth) = blob.get_mut("claudeAiOauth").and_then(Value::as_object_mut) else {
                return false;
            };
            oauth.insert("accessToken".to_owned(), json!(grant.access_token));
            if let Some(token) = &grant.refresh_token {
                oauth.insert("refreshToken".to_owned(), json!(token));
            }
            if let Some(expires_in) = grant.expires_in {
                oauth.insert(
                    "expiresAt".to_owned(),
                    json!((now + expires_in as i64) * millis_per_second()),
                );
            }
            true
        }
        "codex" => {
            {
                let Some(tokens) = blob.get_mut("tokens").and_then(Value::as_object_mut) else {
                    return false;
                };
                tokens.insert("access_token".to_owned(), json!(grant.access_token));
                if let Some(token) = &grant.refresh_token {
                    tokens.insert("refresh_token".to_owned(), json!(token));
                }
                if let Some(token) = &grant.id_token {
                    tokens.insert("id_token".to_owned(), json!(token));
                }
            }
            if blob.get("last_refresh").is_some() {
                if let Some(stamp) = chrono::DateTime::from_timestamp(now, Default::default())
                    .map(|at| at.to_rfc3339())
                {
                    blob["last_refresh"] = json!(stamp);
                }
            }
            true
        }
        "kimi" => {
            let Some(fields) = blob.as_object_mut() else {
                return false;
            };
            fields.insert("access_token".to_owned(), json!(grant.access_token));
            if let Some(token) = &grant.refresh_token {
                fields.insert("refresh_token".to_owned(), json!(token));
            }
            if let Some(expires_in) = grant.expires_in {
                fields.insert("expires_at".to_owned(), json!(now + expires_in as i64));
            }
            true
        }
        _ => false,
    }
}

/// Wipe every string a parsed credential blob holds. Called on every path that
/// parses one, because the parsed copy is as sensitive as the original.
pub(super) fn zeroize_json_strings(value: &mut Value) {
    match value {
        Value::String(text) => text.zeroize(),
        Value::Array(values) => values.iter_mut().for_each(zeroize_json_strings),
        Value::Object(fields) => fields.values_mut().for_each(zeroize_json_strings),
        _ => {}
    }
}
