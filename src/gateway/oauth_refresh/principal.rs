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

/// The address a provider's identity token claims, or `None` when the grant
/// carries none.
///
/// A JWT's payload is its second dot-separated segment, base64url without
/// padding. A grant whose identity token is absent, truncated, not base64, not
/// JSON, or carries no address yields nothing, and the caller leaves the
/// member unattributed rather than recording a guess.
fn identity_claim(token: &str) -> Option<String> {
    let payload = token.split('.').nth(1)?;
    let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .ok()?;
    let claims: Value = serde_json::from_slice(&decoded).ok()?;
    let address = claims
        .get("email")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|address| !address.is_empty())?;
    Some(address.to_owned())
}

/// The account one stored grant belongs to, in that provider's own shape.
///
/// Claude Code records the address beside the grant; Codex and Kimi sign it
/// into an identity token next to the access token. A provider that states
/// neither yields nothing.
pub(in crate::gateway) fn credential_account(blob: &Value, provider: &str) -> Option<String> {
    let stated = |value: Option<&Value>| {
        value
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|address| !address.is_empty())
            .map(str::to_owned)
    };
    match provider {
        "claude-code" => {
            let oauth = blob.get("claudeAiOauth")?;
            stated(oauth.pointer("/account/email_address"))
                .or_else(|| stated(oauth.pointer("/account/emailAddress")))
                .or_else(|| stated(blob.pointer("/account/email_address")))
                .or_else(|| {
                    oauth
                        .get("idToken")
                        .and_then(Value::as_str)
                        .and_then(identity_claim)
                })
        }
        "codex" => {
            let tokens = blob.get("tokens")?;
            tokens
                .get("id_token")
                .and_then(Value::as_str)
                .and_then(identity_claim)
                .or_else(|| stated(tokens.pointer("/account/email")))
        }
        "kimi" => blob
            .get("id_token")
            .and_then(Value::as_str)
            .and_then(identity_claim)
            .or_else(|| stated(blob.get("email"))),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn identity_token(claims: &Value) -> String {
        let payload =
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(claims.to_string().as_bytes());
        format!("header.{payload}.signature")
    }

    /// Codex signs the address into the identity token it stores beside the
    /// access token, which is where the account of every Codex member comes
    /// from.
    #[test]
    fn a_codex_grant_states_its_account() {
        let blob = json!({"tokens": {
            "access_token": "at",
            "id_token": identity_token(&json!({"email": "holder@example.invalid"})),
        }});
        assert_eq!(
            credential_account(&blob, "codex").as_deref(),
            Some("holder@example.invalid")
        );
    }

    /// Claude Code records the address beside the grant rather than in a
    /// token.
    #[test]
    fn a_claude_grant_states_its_account() {
        let blob = json!({"claudeAiOauth": {
            "accessToken": "at",
            "account": {"email_address": "holder@example.invalid"},
        }});
        assert_eq!(
            credential_account(&blob, "claude-code").as_deref(),
            Some("holder@example.invalid")
        );
    }

    /// A grant that states no address yields nothing: an unattributed member
    /// is reported as one, and never labelled from its id or its login row.
    #[test]
    fn a_grant_without_an_address_states_no_account() {
        let blob = json!({"tokens": {"access_token": "at", "id_token": "not-a-token"}});
        assert_eq!(credential_account(&blob, "codex"), None);
        assert_eq!(credential_account(&json!({"tokens": {}}), "codex"), None);
        assert_eq!(credential_account(&json!({}), "claude-code"), None);
    }

    /// An identity token whose claims carry an empty address is the same as
    /// carrying none.
    #[test]
    fn an_empty_address_states_no_account() {
        let blob = json!({"tokens": {
            "id_token": identity_token(&json!({"email": "   "})),
        }});
        assert_eq!(credential_account(&blob, "codex"), None);
    }
}
