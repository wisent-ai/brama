//! Independent provider grants admitted through manual code exchange or console.
//! Stored harness copies retain their provenance and are never adopted as ours.
//! A probe proves a newly admitted grant without rotating its refresh token.

use serde::Serialize;
use serde_json::{json, Value};
use zeroize::Zeroizing;

use super::ManualSignIn;
use crate::gateway::broker;
use crate::subscription_dispatch::probe::probe_once;

/// Where a grant came from, for the verdict's sentence and the journal.
#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case", tag = "origin")]
pub enum Origin {
    /// The provider issued it for a code the operator pasted.
    PastedCode,
}

impl Origin {
    fn sentence(self) -> String {
        match self {
            Origin::PastedCode => "the code the operator pasted".to_owned(),
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

/// Legacy harness-copy provenance. Its presence refuses admission and provider use;
/// only a new independent sign-in can replace the marked grant.
pub const BORROWED_FROM: &str = "brama_borrowed_from";

/// Store the document as this subscription's credential, once it is a grant
/// the refresh path could renew for the provider. What is stored is a grant
/// this gateway signed in for itself; nothing else is accepted, because a
/// second holder of one pair is how both holders lose it.
pub async fn store(
    provider: &str,
    subscription_id: &str,
    document: &str,
    account: Option<&str>,
) -> Result<(), String> {
    let parsed: Value = serde_json::from_str(document)
        .map_err(|error| format!("the grant is not a JSON document: {error}"))?;
    if parsed.get(BORROWED_FROM).is_some() {
        return Err("the grant carries borrowed harness provenance; obtain an independent Brama sign-in instead of adopting or removing its marker".to_owned());
    }
    if !crate::gateway::renewable(&parsed, provider) {
        return Err(format!(
            "the grant is not the `{provider}` document Brama's refresh path reads, or carries no refresh token; Brama cannot keep a grant it cannot renew"
        ));
    }
    let stored = Zeroizing::new(parsed.to_string());
    // The account the grant belongs to is written beside it as the item's
    // `account_ref`: it is what Weles resolves a sign-in from when this
    // grant dies. An imported member that carries none is reported by
    // `/readyz` as `subscription_identity_missing`, and the automatic sign-in
    // that exists to replace a burnt grant cannot start.
    broker::put_subscription_credential_for_account(
        subscription_id,
        provider,
        stored.as_bytes(),
        account,
    )
    .await
    .map_err(|detail| format!("the grant could not be stored: {detail}"))
}

/// Compare grants without the provider's observation-only refresh timestamp.
/// Borrowed provenance is never discarded or accepted as an unchanged own grant.
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
    if incoming.get(BORROWED_FROM).is_some() {
        return false;
    }
    for value in [&mut stored, &mut incoming] {
        if let Some(object) = value.as_object_mut() {
            object.remove("last_refresh");
        }
    }
    stored == incoming
}

/// Whether the item already names the account this grant belongs to. An
/// unchanged grant on an item that names no account is still a write: the
/// account is what Weles signs the subscription in from once the grant dies,
/// and comparing the grant alone leaves an imported member with an
/// unrotated grant and no account through every sweep.
/// A caller that knows no account has nothing to add, and neither has one
/// writing to a local credential store: that store keeps the grant alone,
/// so there is no account beside it to be missing.
async fn account_already_named(
    provider: &str,
    subscription_id: &str,
    account: Option<&str>,
) -> Result<bool, String> {
    let Some(account) = account.map(str::trim).filter(|account| !account.is_empty()) else {
        return Ok(true);
    };
    if broker::local_provider_credentials_enabled() {
        return Ok(true);
    }
    // An account that cannot be read is said, not taken for one that names
    // nobody: the vault that cannot answer this read is the one the store
    // would write to.
    match broker::subscription_account(subscription_id, provider).await {
        Ok(Some(named)) => Ok(named == account),
        Ok(None) => Ok(false),
        Err(error) => Err(format!(
            "which account subscription {subscription_id} names could not be read: {error}"
        )),
    }
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
    // The verdict's sentences name the account the operator gave, and say
    // "the account" when the sign-in named none.
    let who = match &account {
        Some(account) => account.clone(),
        None => "the account".to_owned(),
    };
    // A previously refused independent grant must be proved again, not skipped.
    let disowned = crate::subscription_dispatch::usage::needs_reauthorization(subscription_id);
    if !disowned
        && already_stored(provider, subscription_id, &document).await
        && account_already_named(provider, subscription_id, account.as_deref()).await?
    {
        return Ok(ManualSignIn {
            provider: provider.to_owned(),
            subscription_id: subscription_id.to_owned(),
            account,
            result: "unchanged",
            detail: format!("{who}'s grant from {source} is the one the pool already holds, under {who}'s name; nothing was stored or proved"),
        });
    }
    store(provider, subscription_id, &document, account.as_deref()).await?;
    // The ledger remembers the refusal that disowned the old grant, and the
    // request path leaves a disowned grant alone until a sign-in replaces it.
    // This is that sign-in.
    crate::subscription_dispatch::usage::record_credential_signed_in(subscription_id, provider);
    // Every grant stored here is this gateway's own, so this is always
    // true. It was not while grants could be borrowed: an imported grant was
    // reported as "Brama will refresh it from now on" while `renewal`
    // refused to rotate exactly that grant.
    let refresher = "Brama will refresh it from now on";
    let (result, detail) = match probe_once(subscription_id, provider).await {
        Ok(probe) if probe.ok => (
            "signed_in",
            format!("{who}'s grant from {source} is stored and the provider answered a completion on it; {refresher}"),
        ),
        Ok(probe) => (
            "failed",
            match probe.detail {
                Some(reason) => format!(
                    "the grant from {source} is stored, but the provider would not serve it: {reason}"
                ),
                None => format!(
                    "the grant from {source} is stored, but the provider would not serve it and gave no reason"
                ),
            },
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
    // The origin is part of the journal line; one that does not serialize to
    // fields is refused rather than journaled without it.
    match (record.as_object_mut(), serde_json::to_value(origin)) {
        (Some(record), Ok(Value::Object(origin))) => record.extend(origin),
        (_, Ok(other)) => {
            return Err(format!(
                "the grant from {source} is stored, but its origin is not a set of fields: {other}"
            ))
        }
        (_, Err(error)) => {
            return Err(format!(
                "the grant from {source} is stored, but its origin could not be written: {error}"
            ))
        }
    }
    crate::journal::record_subscription_sign_in(&record).map_err(|error| {
        format!(
            "the grant from {source} is stored and its proof is {}, but the sign-in verdict could not be journaled: {error}",
            verdict.result
        )
    })?;
    Ok(verdict)
}
