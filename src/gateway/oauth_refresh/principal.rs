//! Whose account a stored grant is, according to the grant itself.
//!
//! This is the only source of a member's account that is neither a name nor a
//! person's say-so: the provider signed an identity token into the grant when
//! it issued it, and that token states the address the subscription belongs
//! to. Everything else about a member -- its id, its label, the login row it
//! signs in through -- is a name, and a name is not an account: one Google
//! login row backs both the Claude Code and the Codex subscription of one
//! person, and a member id is a slug nothing can turn back into an address.
//!
//! Read at the one place the gateway already holds a decrypted grant, so no
//! extra read of credential material happens for it, and recorded on the item
//! as `context.account_ref` and `brama:account:` so every later reader --
//! the pool, the CLI, the desktop, and Weles when it signs the account in
//! again -- has the account without opening a secret.
//!
//! Nothing here trusts the token: the signature is not checked, because this
//! is not an authorization decision. It is a claim by the provider about a
//! credential the vault already holds, used to label that credential.

use base64::Engine;
use serde_json::Value;

use super::provider::zeroize_json_strings;
use crate::capability::Secret;

/// An address a field states: its text, trimmed, when it holds any.
fn stated(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|address| !address.is_empty())
        .map(str::to_owned)
}

/// The address a provider's identity token claims.
///
/// A JWT is three dot-separated segments (RFC 7515, compact serialization),
/// and its claims are the second, base64url without padding. A token that
/// claims no address is `Ok(None)`. A token that is not three segments, or
/// whose claims are not base64url or not JSON, is refused with what it
/// lacks: the grant carrying it is damaged, which "states no account" would
/// hide.
pub(crate) fn jwt_claims(token: &str) -> Result<Value, String> {
    let mut segments = token.split('.');
    let (Some(header), Some(payload), Some(signature), None) = (
        segments.next(),
        segments.next(),
        segments.next(),
        segments.next(),
    ) else {
        return Err("the grant's identity token is not three dot-separated segments".to_string());
    };
    if header.is_empty() || payload.is_empty() || signature.is_empty() {
        return Err("the grant's identity token has an empty compact segment".to_string());
    }
    let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .map_err(|error| format!("the grant's identity token claims are not base64url: {error}"))?;
    let claims: Value = serde_json::from_slice(&decoded)
        .map_err(|error| format!("the grant's identity token claims are not JSON: {error}"))?;
    if !claims.is_object() {
        return Err("the grant's identity token claims are not a JSON object".to_string());
    }
    Ok(claims)
}

fn identity_claim(token: &str) -> Result<Option<String>, String> {
    Ok(stated(jwt_claims(token)?.get("email")))
}

/// The address an identity token held in `value` claims, when it holds one.
fn token_claim(value: Option<&Value>) -> Result<Option<String>, String> {
    match value.and_then(Value::as_str) {
        Some(token) => identity_claim(token),
        None => Ok(None),
    }
}

/// The account one stored grant belongs to, in that provider's own shape.
///
/// Claude Code records the address beside the grant; Codex and Kimi sign it
/// into an identity token next to the access token. A provider that states
/// neither yields `Ok(None)`; a damaged identity token is refused.
fn credential_account(blob: &Value, provider: &str) -> Result<Option<String>, String> {
    match provider {
        "claude-code" => {
            let Some(oauth) = blob.get("claudeAiOauth") else {
                return Ok(None);
            };
            let recorded = stated(oauth.pointer("/account/email_address"))
                .or_else(|| stated(oauth.pointer("/account/emailAddress")))
                .or_else(|| stated(blob.pointer("/account/email_address")));
            match recorded {
                Some(account) => Ok(Some(account)),
                None => token_claim(oauth.get("idToken")),
            }
        }
        "codex" => {
            let Some(tokens) = blob.get("tokens") else {
                return Ok(None);
            };
            match token_claim(tokens.get("id_token"))? {
                Some(account) => Ok(Some(account)),
                None => Ok(stated(tokens.pointer("/account/email"))),
            }
        }
        "kimi" => match token_claim(blob.get("id_token"))? {
            Some(account) => Ok(Some(account)),
            None => Ok(stated(blob.get("email"))),
        },
        _ => Ok(None),
    }
}

/// The account a stored grant states, according to the provider that issued
/// it, leaving no parsed copy of the credential behind.
///
/// The one source of a member's account that is not a name: an id, a label
/// and a login row are names, and one Google login row backs both the Claude
/// Code and the Codex subscription of one person. `None` is a grant that
/// states no address; a credential that is not UTF-8, not JSON, or carries a
/// damaged identity token is refused with that cause, since "states no
/// account" would be untrue of it.
pub(in crate::gateway) fn stated_account(
    secret: &Secret,
    provider: &str,
) -> Result<Option<String>, String> {
    let raw = secret
        .expose_utf8()
        .map_err(|error| format!("the stored grant is not UTF-8: {error}"))?;
    match stated_account_text(raw, provider) {
        Ok(stated) => Ok(stated),
        Err(GrantText::NotJson(error)) => {
            Err(format!("the stored grant is not a JSON document: {error}"))
        }
        Err(GrantText::Damaged(detail)) => Err(detail),
    }
}

/// Why a credential text yields no account: it is no JSON grant at all,
/// which a bare API key never is, or it is a grant whose identity token is
/// damaged.
pub(in crate::gateway) enum GrantText {
    NotJson(serde_json::Error),
    Damaged(String),
}

/// [`stated_account`] for a credential text; the caller decides what a text
/// that is no JSON grant means, since a bare API key states no account.
pub(in crate::gateway) fn stated_account_text(
    raw: &str,
    provider: &str,
) -> Result<Option<String>, GrantText> {
    let mut blob: Value = serde_json::from_str(raw).map_err(GrantText::NotJson)?;
    let stated = credential_account(&blob, provider);
    zeroize_json_strings(&mut blob);
    stated.map_err(GrantText::Damaged)
}
