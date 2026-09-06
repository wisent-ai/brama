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

use futures_util::{stream, StreamExt};
use serde_json::{json, Value};
use wisent_errors::{Code, Failure};

use crate::core::failure::{self, POINT_CREDENTIAL_REDEEM};
use crate::gateway::broker::{self, SubscriptionEntry};
use crate::subscription_dispatch::usage::{CredentialState, SubscriptionUsage};
use crate::subscription_dispatch::{plan_usage, usage};

/// The provider named for a subscription whose record carries none, so a row is
/// never keyed on an empty string.
const UNATTRIBUTED: &str = "unattributed";

/// A run that obtained every attempted credential, or an incomplete run.
/// The caller's exit status is read off this, so the two words are fixed here rather
/// than spelled at each return.
const REFRESHED: &str = "refreshed";
const FAILED: &str = "failed";

/// The whole pool as the gateway sees it, read-only.
pub async fn report() -> Value {
    report_scope(None, false).await
}

/// Read free provider reports through the serving process's credential path.
/// This never starts a sign-in or sends a model request.
pub async fn refresh_usage() -> Value {
    report_scope(None, true).await
}

/// The same report, restricted to the caller's authorized agent.
pub async fn report_agent(agent_id: &str, refresh: bool) -> Value {
    report_scope(Some(agent_id), refresh).await
}

async fn report_scope(agent_id: Option<&str>, refresh: bool) -> Value {
    let (entries, mut errors) = inventory(agent_id).await;
    if refresh && errors.is_empty() {
        let attempts: Vec<_> = entries
            .iter()
            .filter(|entry| entry.status == "active" && !crate::journal::is_retired(&entry.id))
            .map(|entry| plan_usage::refresh(&entry.id, &entry.provider))
            .collect();
        if attempts.is_empty() {
            errors.push(
                failure::envelope(
                    "brama.subscriptions.usage",
                    Code::Config,
                    "subscription usage refresh",
                    "no active subscription is available to refresh",
                )
                .with_context("attempted_at_ms", now_ms().to_string()),
            );
        }
        let results = stream::iter(attempts)
            .buffer_unordered(4)
            .collect::<Vec<_>>()
            .await;
        errors.extend(results.into_iter().filter_map(Result::err));
    }
    let mut errors: Vec<Value> = errors
        .into_iter()
        .map(|error| failure_json(&error))
        .collect();
    let observed_at_ms = now_ms();
    let rows = entries
        .iter()
        .map(|entry| {
            let recorded = usage::usage_for(&entry.id);
            let windows = usage::plan_windows(recorded.as_ref());
            if !errors.iter().any(|error| {
                error
                    .pointer("/context/subscription")
                    .and_then(Value::as_str)
                    == Some(entry.id.as_str())
            }) {
                if let Some(error) =
                    reading_error(entry, recorded.as_ref(), &windows, observed_at_ms)
                {
                    errors.push(error);
                }
            }
            if agent_id.is_some() {
                subscription_row(entry, recorded.as_ref(), windows)
            } else {
                let mut row = subscription_row(entry, recorded.as_ref(), windows);
                row["subscription_id"] = json!(entry.id);
                row["state"] = json!(if entry.status == "undiscovered" {
                    "unknown"
                } else if retired(&entry.id, recorded.as_ref()) {
                    "burnt"
                } else {
                    state(recorded.as_ref(), observed_at_ms)
                });
                row["expires_at"] = expires_at(recorded.as_ref());
                row["last_redeem_error"] = last_redeem_error(recorded.as_ref(), observed_at_ms);
                row
            }
        })
        .collect::<Vec<_>>();
    if let Some(detail) = usage::storage_error() {
        errors.push(failure_json(
            &failure::envelope(
                "brama.subscriptions.ledger",
                Code::Config,
                "subscription usage history",
                detail,
            )
            .with_context("attempted_at_ms", observed_at_ms.to_string()),
        ));
    }
    let mut report = json!({
        "ok": errors.is_empty(),
        "observed_at_ms": observed_at_ms,
        "errors": errors,
    });
    if let Some(agent_id) = agent_id {
        report["agentId"] = json!(agent_id);
        report["subscriptions"] = json!(rows);
    } else {
        report["providers"] = json!(rows);
    }
    report
}

fn failure_json(failure: &Failure) -> Value {
    serde_json::from_str(&failure.to_json()).expect("Wisent failure serialization is JSON")
}

async fn inventory(agent_id: Option<&str>) -> (Vec<SubscriptionEntry>, Vec<Failure>) {
    let discovered = match agent_id {
        Some(agent_id) => broker::discover_subscriptions(agent_id).await,
        None => broker::list_all_subscriptions().await,
    };
    let (entries, errors) = match discovered {
        Ok(entries) => (entries, Vec::new()),
        Err(detail) => {
            let mut error = failure::envelope(
                "brama.subscriptions.discovery",
                Code::Config,
                "subscription inventory",
                detail,
            )
            .with_context("attempted_at_ms", now_ms().to_string());
            if let Some(agent_id) = agent_id {
                error = error.with_context("agent", agent_id);
            }
            (Vec::new(), vec![error])
        }
    };
    let mut entries: BTreeMap<_, _> = entries
        .into_iter()
        .map(|entry| (entry.id.clone(), entry))
        .collect();
    // Only the administrator's pool can include historical accounts. A scoped
    // read must never widen an agent's ownership from a shared usage ledger.
    if agent_id.is_none() {
        for (id, recorded) in usage::recorded_subscriptions() {
            entries
                .entry(id.clone())
                .or_insert_with(|| SubscriptionEntry {
                    status: if retired(&id, Some(&recorded)) {
                        "retired"
                    } else {
                        "undiscovered"
                    }
                    .into(),
                    id,
                    provider: named(&recorded.provider),
                    label: None,
                    login_item: None,
                });
        }
    }
    (entries.into_values().collect(), errors)
}

fn reading_error(
    entry: &SubscriptionEntry,
    recorded: Option<&SubscriptionUsage>,
    windows: &usage::PlanWindows,
    now: i64,
) -> Option<Value> {
    if entry.status == "undiscovered" {
        return Some(failure_json(&failure::envelope(
            "brama.subscriptions.discovery",
            Code::Config,
            "one subscription usage report",
            "subscription exists in usage history but was not returned by Skarbiec; its current credential cannot be confirmed",
        )
        .with_context("subscription", &entry.id)
        .with_context("provider", &entry.provider)
        .with_context("attempted_at_ms", now.to_string())));
    }
    if entry.status != "active" || retired(&entry.id, recorded) {
        return None;
    }
    let check = usage::plan_usage_check(recorded);
    if check.is_some_and(|check| !check.ok) {
        if let Some(failure) = recorded.and_then(|recorded| recorded.usage_failure.as_ref()) {
            return Some(failure.clone());
        }
    }
    let credential = recorded.and_then(|recorded| recorded.credential.as_ref());
    let detail = if let Some(cause) = credential
        .filter(|credential| credential.state == CredentialState::NeedsReauthorization)
        .and_then(|credential| credential.cause.as_deref())
    {
        cause
    } else if let Some(check) = check.filter(|check| !check.ok) {
        check
            .detail
            .as_deref()
            .unwrap_or("usage refresh failed without a stated reason")
    } else if windows.stale {
        "the last usage reading is no longer current; refresh usage to obtain a new report"
    } else if windows.limits.is_empty() {
        if check.is_some_and(|check| check.ok)
            && !crate::providers::adapter::publishes_plan_usage(&entry.provider)
        {
            return None;
        }
        if check.is_some() {
            "no current plan windows are available from the last usage reading"
        } else {
            "usage has not been read for this subscription"
        }
    } else {
        return None;
    };
    Some(failure_json(
        &failure::envelope(
            "brama.subscriptions.usage",
            failure::code_for_message(detail, "provider_failure"),
            "one subscription usage report",
            detail,
        )
        .with_context("subscription", &entry.id)
        .with_context("provider", &entry.provider)
        .with_context(
            "attempted_at_ms",
            check.map_or(now, |check| check.attempted_at_ms).to_string(),
        ),
    ))
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

fn named(provider: &str) -> String {
    let trimmed = provider.trim();
    if trimmed.is_empty() {
        return UNATTRIBUTED.to_string();
    }
    trimmed.to_string()
}

/// One account projection shared by the HTTP list, pool, CLI and refresh.
pub fn subscription_view(entry: &SubscriptionEntry) -> Value {
    let recorded = usage::usage_for(&entry.id);
    let windows = usage::plan_windows(recorded.as_ref());
    subscription_row(entry, recorded.as_ref(), windows)
}

fn subscription_row(
    entry: &SubscriptionEntry,
    recorded: Option<&SubscriptionUsage>,
    windows: usage::PlanWindows,
) -> Value {
    json!({
        "id": entry.id,
        "provider": entry.provider,
        "status": entry.status,
        "label": entry.label,
        "login_item": entry.login_item,
        "sign_in": crate::journal::latest_subscription_sign_in(&entry.id),
        "limits": windows.limits,
        "measured": recorded.map(|usage| &usage.measured),
        "block": recorded.and_then(|usage| usage.block.as_ref()),
        "observed_at_ms": recorded.and_then(|usage| usage.updated_at_ms),
        "probe": recorded.and_then(|usage| usage.probe.as_ref()),
        "usage_check": usage::plan_usage_check(recorded),
        "credential": credential_view(entry, recorded),
        "usage_source": windows.source.map(|source| source.as_str()),
        "stale": windows.stale,
    })
}

fn credential_view(entry: &SubscriptionEntry, recorded: Option<&SubscriptionUsage>) -> Value {
    let Some(credential) = recorded.and_then(|usage| usage.credential.as_ref()) else {
        return Value::Null;
    };
    let state = if entry.status != "active" || crate::journal::is_retired(&entry.id) {
        CredentialState::Disabled
    } else {
        credential.state
    };
    json!({
        "state": state.as_str(),
        "cause": credential.cause,
        "recorded_at_ms": credential.recorded_at_ms,
        "expires_at_ms": credential.expires_at_ms,
        "refreshed_at_ms": credential.refreshed_at_ms,
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
    if let Some(detail) = usage::plan_usage_check(Some(usage))
        .filter(|check| !check.ok)
        .and_then(|check| check.detail.as_deref())
    {
        return json!(detail);
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
