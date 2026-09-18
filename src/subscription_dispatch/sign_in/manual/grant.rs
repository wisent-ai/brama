//! A grant Brama did not obtain itself, stored as a subscription's
//! credential and proved without spending the one thing that makes it a
//! grant.
//!
//! A grant reaches Brama three ways now: the provider issues it for a code
//! the operator pasted, a harness on the machine already holds one, or a
//! console hands one over. All three end here, as the one document the
//! refresh path reads for that provider, so the sweep that keeps it alive
//! sees nothing unusual about it.
//!
//! The proof is a probe, not a refresh. A refresh rotates the refresh token
//! and revokes the one it replaced; a grant taken from a harness would then
//! be dead in the harness the moment Brama proved it, and every session the
//! operator has open on that account would fail at its next refresh. A probe
//! spends one minimal completion on the access token both sides hold and
//! rotates nothing; the sweep rotates it later, when the access token nears
//! its end, which is the moment the harness's copy goes stale anyway.

use serde::Serialize;
use serde_json::{json, Value};
use zeroize::Zeroizing;

use super::harness::Harness;
use super::ManualSignIn;
use crate::gateway::broker;
use crate::subscription_dispatch::probe::probe_once;

/// Where a grant came from, for the verdict's sentence and the journal.
#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case", tag = "origin", content = "harness")]
pub enum Origin {
    /// The provider issued it for a code the operator pasted.
    PastedCode,
    /// A harness on the machine held it already.
    Harness(Harness),
    /// A console handed it over as it was.
    Console,
}

impl Origin {
    fn sentence(self) -> String {
        match self {
            Origin::PastedCode => "the code the operator pasted".to_owned(),
            Origin::Harness(harness) => format!("{}, the harness that held it", harness.name()),
            Origin::Console => "the console".to_owned(),
        }
    }
}

/// The Claude grant document in the shape the refresh path reads, from the
/// tokens the provider issued for a pasted code.
pub fn claude_document(access: &str, refresh: &str, expires_at_ms: i64) -> Zeroizing<String> {
    Zeroizing::new(
        json!({
            "claudeAiOauth": {
                "accessToken": access,
                "refreshToken": refresh,
                "expiresAt": expires_at_ms,
                "scopes": super::claude_scopes(),
            }
        })
        .to_string(),
    )
}

/// The field a stored grant carries when a harness on the operator's
/// machine holds the same grant and refreshes it on its own clock.
pub const BORROWED_FROM: &str = "brama_borrowed_from";

/// Store the document as this subscription's credential, once it is a grant
/// the refresh path could renew for the provider. A grant taken from a
/// harness is marked with the harness it came from: Brama must never rotate
/// it, because the harness still holds it and rotates it itself. On
/// 2026-09-17 Brama refreshed two grants it had taken from `omp`; the
/// provider revoked the refresh tokens `omp` held, `omp` recorded
/// `invalid_grant -- Refresh token not found or invalid` on both accounts
/// within the hour, and the operator's own session lost its account.
pub async fn store(
    provider: &str,
    subscription_id: &str,
    document: &str,
    origin: Origin,
    account: Option<&str>,
) -> Result<(), String> {
    let mut parsed: Value = serde_json::from_str(document)
        .map_err(|_| "the grant is not a JSON document".to_owned())?;
    if !crate::gateway::renewable(&parsed, provider) {
        return Err(format!(
            "the grant is not the `{provider}` document Brama's refresh path reads, or carries no refresh token; Brama cannot keep a grant it cannot renew"
        ));
    }
    let stored = match (origin, parsed.as_object_mut()) {
        (Origin::Harness(harness), Some(object)) => {
            object.insert(BORROWED_FROM.into(), json!(harness.name()));
            Zeroizing::new(parsed.to_string())
        }
        _ => Zeroizing::new(document.to_owned()),
    };
    // The account the grant belongs to is written beside it as the item's
    // `account_ref`: it is what Weles resolves a sign-in from when this
    // grant dies, and until 2026-09-18 every imported member carried none —
    // `/readyz` reported each as `subscription_identity_missing`, and the
    // automatic sign-in that exists to replace a burnt grant could not start.
    broker::put_subscription_credential_for_account(
        subscription_id,
        provider,
        stored.as_bytes(),
        account,
    )
    .await
    .map_err(|detail| format!("the grant could not be stored: {detail}"))
}

/// Whether the pool already holds exactly this grant for the subscription:
/// the sweep hands every borrowed grant over on every pass, and a grant the
/// harness has not rotated since is nothing to store or prove again. The
/// borrowed marker and the codex shape's `last_refresh` stamp, written at
/// read time, are not part of the grant.
async fn already_stored(provider: &str, subscription_id: &str, document: &str) -> bool {
    let Ok(current) = broker::redeem_subscription_credential(subscription_id, provider).await
    else {
        return false;
    };
    let Ok(raw) = current.expose_utf8() else {
        return false;
    };
    let (Ok(mut stored), Ok(mut incoming)) = (
        serde_json::from_str::<Value>(raw),
        serde_json::from_str::<Value>(document),
    ) else {
        return false;
    };
    for value in [&mut stored, &mut incoming] {
        if let Some(object) = value.as_object_mut() {
            object.remove(BORROWED_FROM);
            object.remove("last_refresh");
        }
    }
    stored == incoming
}

/// Store the grant, prove it with one minimal completion, and journal the
/// verdict with the operator's reason beside it. A grant the pool already
/// holds unchanged is answered `unchanged` and proved with nothing.
pub async fn adopt(
    provider: &str,
    subscription_id: &str,
    document: Zeroizing<String>,
    account: Option<String>,
    origin: Origin,
    reason: &str,
) -> Result<ManualSignIn, String> {
    if reason.trim().is_empty() {
        return Err("--reason must say why this sign-in is being run".into());
    }
    let source = origin.sentence();
    let who = account.clone().unwrap_or_else(|| "the account".to_owned());
    if already_stored(provider, subscription_id, &document).await {
        return Ok(ManualSignIn {
            provider: provider.to_owned(),
            subscription_id: subscription_id.to_owned(),
            account,
            result: "unchanged",
            detail: format!("{who}'s grant from {source} is the one the pool already holds; nothing was stored or proved"),
        });
    }
    store(
        provider,
        subscription_id,
        &document,
        origin,
        account.as_deref(),
    )
    .await?;
    // The ledger remembers the refusal that disowned the old grant, and the
    // request path leaves a disowned grant alone until a sign-in replaces it.
    // This is that sign-in.
    let borrowed_from = match origin {
        Origin::Harness(harness) => Some(harness.name()),
        _ => None,
    };
    crate::subscription_dispatch::usage::record_credential_signed_in_from(
        subscription_id,
        provider,
        borrowed_from,
    );
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
        account,
        result,
        detail,
    };
    let mut record = json!({
        "provider": verdict.provider,
        "subscription_id": verdict.subscription_id,
        "account": verdict.account,
        "result": verdict.result,
        "detail": verdict.detail,
        "reason": reason.trim(),
    });
    if let (Some(record), Ok(Value::Object(origin))) =
        (record.as_object_mut(), serde_json::to_value(origin))
    {
        record.extend(origin);
    }
    crate::journal::record_subscription_sign_in(&record);
    Ok(verdict)
}
