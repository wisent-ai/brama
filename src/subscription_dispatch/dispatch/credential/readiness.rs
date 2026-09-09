//! Whether one subscription could actually pay for a call, asked the way a
//! request asks it and answered without serving one.

use tracing::warn;

use crate::gateway::broker;
use crate::providers::adapter as provider_registry;
use crate::subscription_dispatch::usage;

use super::super::refusal::envelope::failure_detail;
use super::super::refusal::pool_empty::{
    auth_rejected_summary, bounded_unavailable_summary, no_active_credential_summary,
};

/// Perform, for one subscription, the act a health check cannot infer: redeem
/// its credential and answer in the request path's vocabulary.
///
/// Readiness used to ask whether a subscription contributed a model, which is a
/// declaration -- the catalogue answers it from discovery, and discovery
/// answered `true` all through the morning of 2026-08-18 while every live call
/// was refused. This asks the question a request asks, at the same boundary,
/// through the same broker call, and returns what refused it.
///
/// Redeeming is not the whole question, so the document is reduced the way a
/// request reduces it before this answers -- see
/// [`redeemed_credential_verdict`]. The key that reduction yields is dropped
/// unread: the caller learns that a request could have presented something,
/// and never what.
pub async fn probe_subscription_redemption(
    subscription_id: &str,
    provider: &str,
) -> Result<(), String> {
    if crate::journal::is_retired(subscription_id) {
        return Err(no_active_credential_summary(provider));
    }
    // Checked before redeeming, in the request path's own order: a blocked
    // credential is skipped there without a provider call, so a readiness probe
    // that redeemed it anyway would report a working credential the router
    // refuses to use. An authorization block is named as one, for the same
    // reason the request path names it: `/readyz` is the deploy check, and
    // "wait" and "sign in again" are different instructions to whoever reads it.
    if usage::is_blocked(subscription_id) {
        return Err(if usage::needs_reauthorization(subscription_id) {
            auth_rejected_summary(provider)
        } else {
            bounded_unavailable_summary(provider)
        });
    }
    match broker::subscription_credential(subscription_id, provider).await {
        Ok(credential) => {
            let item = broker::subscription_resource(provider, subscription_id);
            let verdict = match credential.expose_utf8() {
                Ok(secret) => redeemed_credential_verdict(&item, secret),
                Err(error) => Err(format!(
                    "Skarbiec item `{item}` holds bytes that are not valid UTF-8: {error}"
                )),
            };
            drop(credential);
            verdict
        }
        Err(refused) => {
            warn!(
                event = "subscription_redemption_probe_failed",
                subscription = subscription_id,
                provider,
                envelope = %refused.to_json(),
                "{}",
                refused.render()
            );
            Err(failure_detail(&refused))
        }
    }
}

/// Whether a redeemed document is a credential a request could actually
/// present, in the words the router itself would use.
///
/// `Some` from the broker is not the same statement as "the credential
/// redeemed", and reading it as one is how `/readyz` came to answer
/// `redeemable: true` for `brama-sub-wisent-app-codex-secondary` on 2026-09-02
/// while the same gateway's own model call answered `no value at
/// provider:codex:brama-sub-wisent-app-codex-secondary#value` and then `OAuth
/// credential has no refresh token`. The vault row is account metadata: the
/// capability redemption is refused, the read grant hands back the document
/// anyway, it carries no access token, and nothing between there and the
/// provider looked. The catalogue path already said so in its own report --
/// "holds a JSON object with fields [...], which carries no credential" -- so
/// readiness was the only reader being reassured.
///
/// This is the request path's own reduction, not a second opinion: exactly the
/// [`provider_registry::credential_key`] every dispatch calls before it builds
/// an authorization header. The key it returns is dropped unread; the refusal
/// it returns names the item and the field names it looked for and never any
/// credential material.
pub(crate) fn redeemed_credential_verdict(item: &str, secret: &str) -> Result<(), String> {
    provider_registry::credential_key(item, secret).map(drop)
}

#[cfg(test)]
mod tests {
    use super::redeemed_credential_verdict;

    /// The vault coordinate the 2026-09-02 disagreement was about.
    const SECONDARY: &str = "provider:codex:brama-sub-wisent-app-codex-secondary";

    /// The defect: readiness called this row redeemable while the same
    /// gateway's own model call could derive no key from it.
    #[test]
    fn account_metadata_is_not_a_redeemable_credential() {
        let refusal = redeemed_credential_verdict(SECONDARY, r#"{"type":"oauth_account"}"#)
            .expect_err("readiness accepted a document no request could present");
        assert!(
            refusal.contains(SECONDARY),
            "the refusal must name the coordinate the repair is at: {refusal}"
        );
        assert!(
            refusal.contains("carries no credential"),
            "the refusal must say what is wrong with the document: {refusal}"
        );
        assert!(
            refusal.contains("type"),
            "the refusal must name the fields the document does have: {refusal}"
        );
    }

    /// An OAuth grant with an access token is what a request can present, and
    /// the verdict must not leak it.
    #[test]
    fn an_oauth_grant_with_an_access_token_redeems() {
        let document = r#"{"tokens":{"access_token":"not-a-real-token"},"type":"oauth"}"#;
        assert!(redeemed_credential_verdict(SECONDARY, document).is_ok());
    }

    /// A bare key, the other shape the vault holds, still redeems.
    #[test]
    fn a_bare_secret_redeems() {
        assert!(redeemed_credential_verdict(SECONDARY, "sk-not-a-real-key").is_ok());
    }

    /// An empty row is a refusal, not a pass.
    #[test]
    fn an_empty_row_is_refused() {
        assert!(redeemed_credential_verdict(SECONDARY, "   ").is_err());
    }
}
