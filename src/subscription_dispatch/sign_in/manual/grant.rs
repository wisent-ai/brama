//! A Claude grant Brama did not obtain itself, stored as a subscription's
//! credential and proved without spending the one thing that makes it a
//! grant.
//!
//! A grant reaches Brama three ways now: the provider issues it for a code
//! the operator pasted, the operator's harness already holds one, or a
//! console hands one over. All three end here, in the one shape the refresh
//! path reads for `claude-code`, so the sweep that keeps it alive sees
//! nothing unusual about it.
//!
//! The proof is a probe, not a refresh. A refresh rotates the refresh token
//! and revokes the one it replaced; a grant taken from the harness would then
//! be dead in the harness the moment Brama proved it, and every session the
//! operator has open on that account would fail at its next refresh. A probe
//! spends one minimal completion on the access token both sides hold and
//! rotates nothing; the sweep rotates it later, when the access token nears
//! its end, which is the moment the harness's copy goes stale anyway.

use serde::{Deserialize, Serialize};
use serde_json::json;
use zeroize::Zeroizing;

use super::{manual_provider, ManualSignIn};
use crate::gateway::broker;
use crate::subscription_dispatch::probe::probe_once;

/// One OAuth grant as the provider issues it: the two tokens and when the
/// first of them ends, in milliseconds since the epoch.
#[derive(Clone, Deserialize)]
pub struct Grant {
    pub access_token: Zeroizing<String>,
    pub refresh_token: Zeroizing<String>,
    pub expires_at_ms: i64,
    /// The account the provider or the harness named for it, when known.
    #[serde(default)]
    pub account: Option<String>,
}

impl Grant {
    /// The vault document the refresh path reads for `claude-code`.
    pub fn document(&self, provider: &str) -> Result<Zeroizing<String>, String> {
        let config = manual_provider(provider).ok_or_else(|| {
            format!("a manual sign-in is defined for claude-code; `{provider}` is not it")
        })?;
        Ok(Zeroizing::new(
            json!({
                "claudeAiOauth": {
                    "accessToken": &*self.access_token,
                    "refreshToken": &*self.refresh_token,
                    "expiresAt": self.expires_at_ms,
                    "scopes": config.scopes,
                }
            })
            .to_string(),
        ))
    }
}

/// Where a grant came from, for the verdict's sentence.
#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Origin {
    /// The provider issued it for a code the operator pasted.
    PastedCode,
    /// The operator's harness held it already.
    Harness,
    /// A console handed it over as it was.
    Console,
}

/// Store the grant as this subscription's credential.
pub async fn store(provider: &str, subscription_id: &str, grant: &Grant) -> Result<(), String> {
    if grant.access_token.is_empty() {
        return Err("the grant carries no access token".into());
    }
    if grant.refresh_token.is_empty() {
        return Err(
            "the grant carries no refresh token; Brama cannot keep a grant it cannot renew".into(),
        );
    }
    let document = grant.document(provider)?;
    broker::put_subscription_credential(subscription_id, provider, document.as_bytes())
        .await
        .map_err(|detail| format!("the grant could not be stored: {detail}"))
}

/// Store the grant, prove it with one minimal completion, and journal the
/// verdict with the operator's reason beside it.
pub async fn adopt(
    provider: &str,
    subscription_id: &str,
    grant: Grant,
    origin: Origin,
    reason: &str,
) -> Result<ManualSignIn, String> {
    if reason.trim().is_empty() {
        return Err("--reason must say why this sign-in is being run".into());
    }
    store(provider, subscription_id, &grant).await?;
    // The ledger remembers the refusal that disowned the old grant, and the
    // request path leaves a disowned grant alone until a sign-in replaces it.
    // This is that sign-in.
    crate::subscription_dispatch::usage::record_credential_signed_in(subscription_id, provider);
    let source = match origin {
        Origin::PastedCode => "the code the operator pasted",
        Origin::Harness => "the operator's harness",
        Origin::Console => "the console",
    };
    let who = grant
        .account
        .clone()
        .unwrap_or_else(|| "the account".to_owned());
    let (result, detail) = match probe_once(subscription_id, provider).await {
        Ok(probe) if probe.ok => (
            "signed_in",
            format!("{who}'s grant from {source} is stored and the provider answered a completion on it; Brama will refresh it from now on"),
        ),
        Ok(probe) => (
            "failed",
            format!(
                "the grant from {source} is stored, but the provider would not serve it: {}",
                probe.detail.unwrap_or_else(|| "the provider gave no reason".to_owned())
            ),
        ),
        Err(detail) => (
            "failed",
            format!("the grant from {source} is stored, but the proof could not be spent: {detail}"),
        ),
    };
    let verdict = ManualSignIn {
        provider: provider.to_owned(),
        subscription_id: subscription_id.to_owned(),
        account: grant.account,
        result,
        detail,
    };
    crate::journal::record_subscription_sign_in(&json!({
        "provider": verdict.provider,
        "subscription_id": verdict.subscription_id,
        "account": verdict.account,
        "result": verdict.result,
        "detail": verdict.detail,
        "origin": origin,
        "reason": reason.trim(),
    }));
    Ok(verdict)
}
