//! What the subscription pool holds, and one operator-driven refresh of it.
//!
//! Browser automation across the company stopped for most of a working day
//! because this pool was empty. Both codex subscription credentials were burnt,
//! every `best`-aliased call answered `429 subscription_unavailable`, and the
//! only way to learn which of the two facts was true -- an exhausted plan or a
//! disowned grant -- was to grep `brama-always-on.err` for the code and read the
//! timestamps by hand. The gateway had known since its first refresh sweep: the
//! ledger already carried `needs_reauthorization` against both grants with the
//! provider's own sentence beside them, and no command in the product would say
//! it out loud. These two commands say it.
//!
//! Reading and acting are separated on purpose. [`report`] contacts no provider
//! and redeems no capability -- it joins the deployment's subscription listing to
//! the ledger and states what is already recorded -- so it is safe to run
//! against a gateway that is serving traffic. [`refresh_provider`] rotates a
//! grant, which mutates state every later request depends on, so it demands a
//! reason and leaves an audit record beside the state it changed.
//!
//! Neither can print credential material. The listing reads the ledger, which
//! has never held any, and the refresh drops the credential it obtains without
//! looking at it: what it reports is the verdict, not the grant.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use crate::core::failure::POINT_CREDENTIAL_REDEEM;
use crate::gateway::broker;
use crate::subscription_dispatch::usage::{self, CredentialState, SubscriptionUsage};

/// The provider named for a subscription whose record carries none, so a row is
/// never keyed on an empty string.
const UNATTRIBUTED: &str = "unattributed";

/// A run that obtained at least one credential, and one that obtained none. The
/// caller's exit status is read off this, so the two words are fixed here rather
/// than spelled at each return.
const REFRESHED: &str = "refreshed";
const FAILED: &str = "failed";

/// The whole pool as the gateway sees it, read-only.
pub async fn report() -> Value {
    let now_ms = now_ms();
    let providers = pool()
        .await
        .into_iter()
        .map(|(provider, subscription_id, recorded)| {
            row(&provider, &subscription_id, recorded.as_ref(), now_ms)
        })
        .collect::<Vec<_>>();
    json!({"providers": providers})
}

/// Refresh every grant this deployment holds for one provider, because an
/// operator asked for it now instead of waiting for the sweep.
///
/// The sweep and this share one code path -- the forced form of what the timer
/// calls -- so a grant cannot come back alive here and dead there, and every
/// refusal is classified and recorded in the ledger exactly once. The only
/// difference is the skew window: a burnt credential is never due, and a timer
/// that will not try it is precisely what leaves the pool empty until somebody
/// signs in.
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
    let candidates = candidates(provider).await;
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
        if refreshed > usize::default() {
            REFRESHED
        } else {
            FAILED
        },
        detail(provider, attempted, refreshed, unreadable, &refusals),
    ))
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
async fn candidates(provider: &str) -> Vec<String> {
    pool()
        .await
        .into_iter()
        .filter(|(row_provider, _, _)| row_provider == provider)
        .filter(|(_, subscription_id, recorded)| !retired(subscription_id, recorded.as_ref()))
        .map(|(_, subscription_id, _)| subscription_id)
        .collect()
}

/// Every subscription this deployment has: the broker's listing, widened by
/// every subscription the ledger holds a record for.
///
/// The union is the point. The listing alone misses a subscription whose vault
/// item was removed while the gateway still holds a burnt grant for it, and it
/// needs the entitlements router on `PATH` -- which is exactly what a shell
/// diagnosing an empty pool may not have, and an empty answer there would read
/// as an empty pool. The ledger alone misses a subscription nothing has used
/// yet. Together they are what the gateway routes over.
async fn pool() -> Vec<(String, String, Option<SubscriptionUsage>)> {
    let mut recorded = usage::recorded_subscriptions();
    let mut rows: BTreeMap<(String, String), Option<SubscriptionUsage>> = BTreeMap::new();
    for entry in broker::list_all_subscriptions().await {
        let ledger_record = recorded.remove(&entry.id);
        rows.insert((named(&entry.provider), entry.id), ledger_record);
    }
    for (subscription_id, ledger_record) in recorded {
        rows.insert(
            (named(&ledger_record.provider), subscription_id),
            Some(ledger_record),
        );
    }
    rows.into_iter()
        .map(|((provider, subscription_id), ledger_record)| {
            (provider, subscription_id, ledger_record)
        })
        .collect()
}

fn named(provider: &str) -> String {
    let trimmed = provider.trim();
    if trimmed.is_empty() {
        return UNATTRIBUTED.to_string();
    }
    trimmed.to_string()
}

/// One pool row: which account it is, whether its credential works, until when,
/// and the last thing that refused it.
fn row(
    provider: &str,
    subscription_id: &str,
    recorded: Option<&SubscriptionUsage>,
    now_ms: i64,
) -> Value {
    json!({
        "provider": provider,
        "subscription_id": subscription_id,
        "state": state(recorded, now_ms),
        "expires_at": expires_at(recorded),
        "last_redeem_error": last_redeem_error(recorded, now_ms),
    })
}

/// Where one grant stands, in the four words an operator acts on.
///
/// `burnt` covers both recorded dead states -- a grant the provider disowned and
/// a subscription somebody retired -- because the pool serves neither and no
/// retry changes either; `last_redeem_error` says which of the two it was.
/// `unknown` is the honest answer for a subscription whose grant nothing has
/// ever looked at, which is not the same statement as a working one; that
/// distinction is exactly what the burnt-codex morning needed and did not have.
fn state(recorded: Option<&SubscriptionUsage>, now_ms: i64) -> &'static str {
    let Some(credential) = recorded.and_then(|usage| usage.credential.as_ref()) else {
        return "unknown";
    };
    match credential.state {
        CredentialState::NeedsReauthorization | CredentialState::Disabled => "burnt",
        // An API key states no expiry and never has one, so an absent instant is
        // a live credential rather than an unknown one.
        CredentialState::Active => match credential.expires_at_ms {
            Some(expires_at_ms) if expires_at_ms <= now_ms => "expired",
            _ => "live",
        },
    }
}

/// The provider's stated expiry as an instant a human reads, or `null` when the
/// credential states none.
fn expires_at(recorded: Option<&SubscriptionUsage>) -> Value {
    recorded
        .and_then(|usage| usage.credential.as_ref())
        .and_then(|credential| credential.expires_at_ms)
        .and_then(chrono::DateTime::from_timestamp_millis)
        .map(|at| json!(at.to_rfc3339()))
        .unwrap_or(Value::Null)
}

/// The refusal standing in this credential's way, in the words of whatever
/// refused it.
///
/// Three records can hold one and the order is what a repair needs. The
/// credential's own cause outranks everything: it is set only while the grant is
/// refused, and a sign-in is the only thing that clears it. A block still in
/// force is next -- the provider said this account is out of quota, which stops
/// a redemption just as effectively for as long as it lasts. A failed check is
/// last, being a verdict about the account rather than about the grant.
///
/// A lapsed block is deliberately not reported. It was true an hour ago and is
/// not now, and a stale refusal printed beside a `live` grant is what sends an
/// operator looking for a sign-in that nothing needs.
fn last_redeem_error(recorded: Option<&SubscriptionUsage>, now_ms: i64) -> Value {
    let Some(usage) = recorded else {
        return Value::Null;
    };
    if let Some(cause) = usage
        .credential
        .as_ref()
        .and_then(|credential| credential.cause.as_deref())
    {
        return json!(cause);
    }
    if let Some(block) = usage
        .block
        .as_ref()
        .filter(|block| block.blocked_until_ms > now_ms)
    {
        return json!(block.reason);
    }
    usage
        .probe
        .as_ref()
        .filter(|probe| !probe.ok)
        .and_then(|probe| probe.detail.as_deref())
        .map_or(Value::Null, |detail| json!(detail))
}

/// Whether this subscription was deliberately taken out of the pool.
fn retired(subscription_id: &str, recorded: Option<&SubscriptionUsage>) -> bool {
    crate::journal::is_retired(subscription_id)
        || recorded
            .and_then(|usage| usage.credential.as_ref())
            .is_some_and(|credential| credential.state == CredentialState::Disabled)
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}
