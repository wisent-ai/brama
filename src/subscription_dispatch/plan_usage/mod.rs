//! Read what each subscription's plan has left from the provider's own usage
//! report, and keep that reading current without spending any of the plan.
//!
//! A provider that rations a subscription publishes a report of how much of the
//! ration is gone. Reading it costs a request and no quota: no completion, no
//! output tokens, nothing that appears in the account's own usage. That is the
//! whole difference from what this gateway did before, which was to buy the
//! answer with one small completion per subscription every quarter hour --
//! spending the thing it was trying to measure, on a timer, forever.
//!
//! Three rules hold here, and each of them is about the report being cheap but
//! not free of consequence. It is read at most once per subscription per cache
//! window, because provider usage endpoints may rate-limit per source address
//! and several accounts on one host share that address. Each
//! subscription's window is spread by up to a quarter either way, derived from
//! its own id, so those accounts never come due in the same second. And a
//! failed read never replaces a good reading: the last one is kept and marked
//! stale immediately, so a row says both what it last knew and why that reading
//! is no longer current instead of blanking over one bad minute upstream.
//!
//! A provider credential with no supported free usage-report endpoint is
//! recorded that way. The vendor may expose separately privileged billing APIs;
//! Brama states only what this credential can read without inference.
//!
//! This file owns the read itself: which endpoint the credential is allowed to
//! ask, the one retry a rejected token earns, what each outcome writes to the
//! ledger, and the answer given to the three audiences that ask. When that read
//! is looked for again lives in `sweep`, sharing a read already under way lives
//! in `in_flight`, and what a refusal is called lives in `refusal`.

mod in_flight;
mod refusal;
mod sweep;

use futures_util::{stream, StreamExt};
use serde_json::Value;
use tracing::{info, warn};
use wisent_errors::{Code, Failure};

use crate::core::failure;
use crate::gateway::broker;
use crate::providers::adapter::{self as provider_registry, PlanUsage};
use crate::subscription_dispatch::{pool, usage};

use in_flight::{join_refresh, shared_refresh, RefreshResult};
use refusal::{
    invalid_credential_failure, is_provider_authentication, provider_failure, UNPUBLISHED_DETAIL,
};

// The one name the rest of the crate calls that no longer lives in this file,
// re-exported by itself so `plan_usage::spawn` keeps working and nothing else
// travels with it.
pub use sweep::spawn;

async fn refresh_once(subscription_id: &str, provider: &str) -> RefreshResult {
    if !provider_registry::publishes_plan_usage(provider) {
        usage::record_plan_usage_unpublished(subscription_id, provider, UNPUBLISHED_DETAIL)?;
        info!(
            event = "plan_usage_unpublished",
            subscription = %subscription_id,
            provider = %provider,
            "Brama has no supported free usage-report endpoint for this provider credential"
        );
        return Ok(());
    }

    let item = broker::subscription_resource(provider, subscription_id);
    let credential = broker::subscription_credential(subscription_id, provider).await?;
    let token = credential
        .expose_utf8()
        .map_err(|_| invalid_credential_failure(subscription_id, provider, &item))?;
    let first = provider_registry::read_plan_usage(provider, &item, token).await;
    let outcome = match first {
        PlanUsage::Refused(detail)
            if is_provider_authentication(&detail) && broker::supports_oauth_refresh(provider) =>
        {
            let rejected = provider_failure(subscription_id, provider, detail);
            drop(credential);
            let fresh = broker::refresh_subscription_credential(subscription_id, provider)
                .await
                .map_err(|refused| refused.caused_by(rejected.clone()))?;
            let fresh_token = fresh.expose_utf8().map_err(|_| {
                invalid_credential_failure(subscription_id, provider, &item).caused_by(rejected)
            })?;
            provider_registry::read_plan_usage(provider, &item, fresh_token).await
        }
        outcome => outcome,
    };

    match outcome {
        PlanUsage::Report(readings) => {
            let windows = readings.len();
            usage::record_plan_usage(subscription_id, provider, &readings)?;
            info!(
                event = "plan_usage_recorded",
                subscription = %subscription_id,
                provider = %provider,
                windows,
                "read one provider usage report; no quota was spent"
            );
            Ok(())
        }
        PlanUsage::Unpublished => {
            usage::record_plan_usage_unpublished(subscription_id, provider, UNPUBLISHED_DETAIL)
        }
        PlanUsage::Refused(detail) => {
            warn!(
                event = "plan_usage_refused",
                subscription = %subscription_id,
                provider = %provider,
                detail = %detail,
                "the provider would not state this subscription's usage; the last good reading \
                 is retained and stale"
            );
            Err(provider_failure(subscription_id, provider, detail))
        }
    }
}

/// Read one subscription's provider-only usage report and record the outcome.
///
/// Concurrent callers join the same bounded operation and receive its same
/// success or detailed failure.
pub async fn refresh(subscription_id: &str, provider: &str) -> Result<(), Failure> {
    join_refresh(
        shared_refresh(subscription_id, provider),
        subscription_id,
        provider,
    )
    .await
}

/// Subscription plan usage: what every plan the caller proved it owns has
/// left, from this ledger and each provider's own usage report.
///
/// One capability for three audiences. Which accounts it answers about follows
/// from the [`PoolScope`](pool::PoolScope) the caller proved, so the console,
/// an account holder and a signed agent are answered by one implementation
/// narrowed three ways instead of by four refreshes that can disagree. It
/// spends no plan quota: reading a report costs a request and no completion,
/// and nothing here starts a sign-in.
///
/// An unreadable inventory stops the reads. A listing that failed says nothing
/// about which accounts exist, and asking a provider about accounts nobody
/// confirmed puts the wrong question to it; the failure is reported instead. A
/// read that fails leaves the last good reading in place, marked stale, with
/// its refusal in `errors` -- so an incomplete answer says which account it is
/// incomplete about rather than reporting a missing measurement as zero usage.
pub async fn report(scope: &pool::PoolScope) -> Value {
    let (entries, mut errors) = pool::inventory(scope).await;
    if errors.is_empty() {
        let attempts: Vec<_> = entries
            .iter()
            .filter(|entry| entry.status == "active" && !crate::journal::is_retired(&entry.id))
            .map(|entry| refresh(&entry.id, &entry.provider))
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
        // Bounded fan-out: several accounts on one host share one source
        // address, and a provider that rate-limits per address refuses the
        // burst that asking about all of them at once would produce.
        let results = stream::iter(attempts)
            .buffer_unordered(4)
            .collect::<Vec<_>>()
            .await;
        errors.extend(results.into_iter().filter_map(Result::err));
    }
    pool::document(scope, &entries, errors)
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}
