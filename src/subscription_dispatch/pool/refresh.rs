//! Rotating the grants this deployment holds, because an operator asked now.
//!
//! Reading the pool and acting on it are separated on purpose: the document
//! beside this contacts no provider, while everything here mutates state that
//! every later request depends on. So each command demands a reason, leaves an
//! audit record beside the state it changed, and reports the verdict rather
//! than the credential -- what it obtains is dropped unread.
//!
//! The sweep and these share one code path, the forced form of what the timer
//! calls, so a grant cannot come back alive here and dead there.

use serde_json::{json, Value};

use crate::core::failure::POINT_CREDENTIAL_REDEEM;
use crate::gateway::broker;
use crate::subscription_dispatch::usage;

use super::account::retired;

/// A run that obtained every attempted credential, or an incomplete run.
/// The caller's exit status is read off this, so the two words are fixed here rather
/// than spelled at each return.
const REFRESHED: &str = "refreshed";
const FAILED: &str = "failed";

/// Refresh every grant this deployment holds for one provider, because an
/// operator asked for it now instead of waiting for the sweep.
///
/// The only difference from the timer's own call is the skew window: a burnt
/// credential is never due, and a timer that will not try it is precisely what
/// leaves the pool empty until somebody signs in.
pub async fn refresh_provider(provider: &str, reason: &str) -> Result<Value, String> {
    let provider = provider.trim();
    if provider.is_empty() {
        return Err("a provider is required".into());
    }
    let reason = reason.trim();
    if reason.is_empty() {
        return Err("--reason must say why this refresh is being run".into());
    }
    // The pool is enumerated before anything is said about the provider, so a
    // name this deployment holds no subscription for -- a typo included -- is
    // told that rather than being described as an API-key provider it may not be
    // at all.
    let candidates = candidates(provider).await?;
    if candidates.is_empty() {
        return Ok(verdict(
            provider,
            reason,
            usize::default(),
            FAILED,
            format!(
                "no usable `{provider}` subscription is in this deployment's pool, so no \
                 credential source is configured to refresh: one has to be signed in and stored \
                 in the vault before this command has anything to act on"
            ),
        ));
    }
    if !broker::supports_oauth_refresh(provider) {
        return Ok(verdict(
            provider,
            reason,
            usize::default(),
            FAILED,
            format!(
                "`{provider}` subscription credentials are API keys rather than OAuth grants, so \
                 no refresh path exists for them: replacing one means storing a new credential in \
                 the vault"
            ),
        ));
    }
    let attempted = candidates.len();
    let mut refreshed = usize::default();
    let mut refusals = Vec::new();
    let mut unreadable = usize::default();
    for subscription_id in &candidates {
        match broker::refresh_subscription_credential(subscription_id, provider).await {
            // Dropped unread. This command reports that a credential now exists,
            // never what it is, and the ledger already holds the new expiry.
            Ok(credential) => {
                drop(credential);
                refreshed = refreshed.saturating_add(1);
            }
            Err(refused) => {
                // A refusal raised at the redeem point never reached the
                // provider: nothing produced a credential to refresh. It is
                // counted apart because the repair is a different one, and
                // because a shell without the launcher's capability environment
                // hits it for every subscription at once.
                if refused.failure_point == POINT_CREDENTIAL_REDEEM {
                    unreadable = unreadable.saturating_add(1);
                }
                refusals.push(format!(
                    "{subscription_id}: {}",
                    refused
                        .detail
                        .as_deref()
                        .unwrap_or("refused without a stated reason")
                ));
            }
        }
    }
    Ok(verdict(
        provider,
        reason,
        attempted,
        if refreshed == attempted {
            REFRESHED
        } else {
            FAILED
        },
        detail(provider, attempted, refreshed, unreadable, &refusals),
    ))
}

/// Refresh exactly the subscription a sign-in replaced.
///
/// Provider-wide refresh is useful to an operator, but it is not proof for an
/// automatic repair: another healthy account on the same provider could make
/// that aggregate answer `refreshed` while the requested account stayed dead.
pub async fn refresh_subscription(
    provider: &str,
    subscription_id: &str,
    reason: &str,
) -> Result<Value, String> {
    let provider = provider.trim();
    let subscription_id = subscription_id.trim();
    let reason = reason.trim();
    if provider.is_empty() || subscription_id.is_empty() {
        return Err("a provider and subscription id are required".into());
    }
    if reason.is_empty() {
        return Err("--reason must say why this refresh is being run".into());
    }
    if !candidates(provider)
        .await?
        .iter()
        .any(|candidate| candidate == subscription_id)
    {
        return Ok(verdict(
            provider,
            reason,
            usize::default(),
            FAILED,
            format!(
                "`{subscription_id}` is not an active `{provider}` subscription in this deployment"
            ),
        ));
    }
    if !broker::supports_oauth_refresh(provider) {
        return Ok(verdict(
            provider,
            reason,
            usize::default(),
            FAILED,
            format!("`{provider}` credentials have no OAuth refresh path"),
        ));
    }
    match broker::refresh_subscription_credential(subscription_id, provider).await {
        Ok(credential) => {
            drop(credential);
            Ok(verdict(
                provider,
                reason,
                1,
                REFRESHED,
                format!("refreshed `{subscription_id}`"),
            ))
        }
        Err(refused) => {
            let detail = refused
                .detail
                .as_deref()
                .unwrap_or("refused without a stated reason");
            Ok(verdict(
                provider,
                reason,
                1,
                FAILED,
                format!("`{subscription_id}`: {detail}"),
            ))
        }
    }
}

/// One verdict, in the shape the caller prints and the audit record keeps.
///
/// Both are written here so a record cannot say something the operator was never
/// told, and so an attempt that found nothing to do is audited too: afterwards
/// the question is who ran this and what the product answered.
fn verdict(provider: &str, reason: &str, attempted: usize, result: &str, detail: String) -> Value {
    crate::journal::record_subscription_refresh(provider, reason, result, attempted, &detail);
    json!({
        "provider": provider,
        "attempted": attempted,
        "result": result,
        "detail": detail,
    })
}

/// What a run of refreshes came to, in one sentence an operator can act on.
///
/// The refusals are quoted in the provider's own words rather than summarised.
/// `invalid_grant` and "this account is over its plan" are the same count and
/// two entirely different repairs, and a count is what made the difference
/// invisible for five days.
fn detail(
    provider: &str,
    attempted: usize,
    refreshed: usize,
    unreadable: usize,
    refusals: &[String],
) -> String {
    let mut detail = if refreshed > usize::default() {
        format!("refreshed {refreshed} of {attempted} `{provider}` grants")
    } else {
        format!("refreshed no `{provider}` grant out of {attempted} tried")
    };
    for refusal in refusals {
        detail.push_str("; ");
        detail.push_str(refusal);
    }
    // Said plainly and once, because it is not a provider refusal at all: the
    // capability ids and request-signing identity a redemption needs live in the
    // serving process's environment, so a subcommand started by hand elsewhere
    // reports this for every subscription and none of them is broken.
    if unreadable == attempted {
        detail.push_str(&format!(
            "; no usable credential source is configured for `{provider}` in this environment: no \
             capability and no read grant produced a credential to refresh, so run this where the \
             launcher installed the gateway's own capability environment"
        ));
    }
    detail
}

/// The subscriptions a refresh for this provider should act on.
///
/// A retired subscription is left out. Somebody took it out of the pool
/// deliberately, and rotating its grant would put back what they removed.
async fn candidates(provider: &str) -> Result<Vec<String>, String> {
    Ok(broker::list_all_subscriptions()
        .await?
        .into_iter()
        .filter(|entry| entry.provider == provider && entry.status == "active")
        .filter(|entry| !retired(&entry.id, usage::usage_for(&entry.id).as_ref()))
        .map(|entry| entry.id)
        .collect())
}
