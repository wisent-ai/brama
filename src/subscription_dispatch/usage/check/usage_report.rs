//! What the provider's own usage report writes to the ledger, and what a
//! refused read leaves behind.
//!
//! This is the ordinary way a plan window arrives: it costs no quota, so it
//! runs on a timer, and its readings land in exactly the map real traffic
//! writes to. Four outcomes are worth telling apart, which is why they are four
//! calls rather than one with a flag -- windows were published, the report was
//! read and named no window, Brama supports no free report for this credential
//! at all, and the read was refused.
//!
//! Each of them also has to survive a ledger that cannot be written. The
//! observation is kept in this process either way, the storage failure is
//! returned to the caller as a fleet envelope, and the refusal is recorded
//! against the subscription so a report in this process still says why the row
//! is not current.

use wisent_errors::{Code, Failure};

use crate::core::failure;
use crate::subscription_dispatch::usage::{now_ms, with_ledger_memory, write_ledger, UsageSource};
use crate::types::LimitReading;

use super::{CheckSource, Probe};

const POINT_USAGE_LEDGER_PERSIST: &str = "brama.subscription-usage.ledger-persist";
const IMPACT_PLAN_USAGE: &str = "this subscription's current usage report";

fn plan_usage_storage_failure(subscription_id: &str, provider: &str, detail: String) -> Failure {
    failure::envelope(
        POINT_USAGE_LEDGER_PERSIST,
        Code::Config,
        IMPACT_PLAN_USAGE,
        detail,
    )
    .with_context("subscription", subscription_id)
    .with_context("provider", provider)
}

fn failure_value(failure: &Failure) -> serde_json::Value {
    serde_json::from_str(&failure.to_json()).expect("Wisent failure serialization is JSON")
}

/// Usage failures use the fleet envelope's detail bound so endpoint, status,
/// and provider reason survive together.
fn bounded_usage_detail(detail: Option<&str>) -> Option<String> {
    detail
        .map(str::trim)
        .filter(|detail| !detail.is_empty())
        .map(|detail| {
            detail
                .chars()
                .take(wisent_errors::DETAIL_LIMIT)
                .collect::<String>()
        })
}

fn remember_plan_usage_failure(
    subscription_id: &str,
    provider: &str,
    attempted_at_ms: i64,
    refused: &Failure,
) {
    let detail = bounded_usage_detail(refused.detail.as_deref());
    let envelope = failure_value(refused);
    with_ledger_memory(|ledger| {
        let entry = ledger
            .subscriptions
            .entry(subscription_id.to_string())
            .or_default();
        entry.provider = provider.to_string();
        entry.updated_at_ms = Some(attempted_at_ms);
        entry.plan_usage_checked_at_ms = Some(attempted_at_ms);
        entry.usage_check = Some(Probe {
            attempted_at_ms,
            ok: false,
            detail,
            source: Some(CheckSource::UsageReport),
        });
        entry.usage_failure = Some(envelope);
    });
}

/// Record what the provider's own usage report said about one subscription.
///
/// This is the ordinary path now: it costs no quota, so it runs on a timer, and
/// the readings land in exactly the map real traffic writes to, keyed by the same
/// limit ids. A report that carried no window is still a successful check --
/// which is what tells a reader that the blank row is the provider's answer and
/// not a broken credential.
pub fn record_plan_usage(
    subscription_id: &str,
    provider: &str,
    readings: &[LimitReading],
) -> Result<(), Failure> {
    let now = now_ms();
    let (_, stored) = write_ledger(|ledger| {
        let entry = ledger
            .subscriptions
            .entry(subscription_id.to_string())
            .or_default();
        entry.provider = provider.to_string();
        entry.updated_at_ms = Some(now);
        entry.plan_usage_checked_at_ms = Some(now);
        for reading in readings {
            entry
                .limits
                .insert(reading.limit_id.clone(), reading.clone());
        }
        if !readings.is_empty() {
            entry.usage_source = Some(UsageSource::Provider);
        }
        entry.usage_check = Some(Probe {
            attempted_at_ms: now,
            ok: true,
            detail: None,
            source: Some(CheckSource::UsageReport),
        });
        entry.usage_failure = None;
    });
    match stored {
        Ok(()) => Ok(()),
        Err(error) => {
            let refused = plan_usage_storage_failure(subscription_id, provider, error);
            remember_plan_usage_failure(subscription_id, provider, now, &refused);
            Err(refused)
        }
    }
}

/// Record that Brama supports no free usage-report endpoint for this provider
/// credential.
///
/// This is a fact about Brama's credential-scoped integration, not a claim that
/// the vendor has no separately privileged billing API.
pub fn record_plan_usage_unpublished(
    subscription_id: &str,
    provider: &str,
    detail: &str,
) -> Result<(), Failure> {
    let now = now_ms();
    let detail = bounded_usage_detail(Some(detail));
    let (_, stored) = write_ledger(|ledger| {
        let entry = ledger
            .subscriptions
            .entry(subscription_id.to_string())
            .or_default();
        entry.provider = provider.to_string();
        entry.updated_at_ms = Some(now);
        entry.plan_usage_checked_at_ms = Some(now);
        entry.usage_check = Some(Probe {
            attempted_at_ms: now,
            ok: true,
            detail,
            source: Some(CheckSource::UsageReport),
        });
        entry.usage_failure = None;
    });
    match stored {
        Ok(()) => Ok(()),
        Err(error) => {
            let refused = plan_usage_storage_failure(subscription_id, provider, error);
            remember_plan_usage_failure(subscription_id, provider, now, &refused);
            Err(refused)
        }
    }
}

/// Record one failed free usage attempt with its complete Wisent envelope.
///
/// The readings already stored are left exactly where they are. A failure is a
/// reason the row is not current, not evidence that the last good reading was
/// wrong, and replacing it with nothing would blank a screen over one bad
/// attempt.
pub fn record_plan_usage_failure(
    subscription_id: &str,
    provider: &str,
    refused: &Failure,
) -> Result<(), Failure> {
    let now = now_ms();
    let detail = bounded_usage_detail(refused.detail.as_deref());
    let envelope = failure_value(refused);
    let (_, stored) = write_ledger(|ledger| {
        let entry = ledger
            .subscriptions
            .entry(subscription_id.to_string())
            .or_default();
        entry.provider = provider.to_string();
        entry.updated_at_ms = Some(now);
        entry.plan_usage_checked_at_ms = Some(now);
        entry.usage_check = Some(Probe {
            attempted_at_ms: now,
            ok: false,
            detail,
            source: Some(CheckSource::UsageReport),
        });
        entry.usage_failure = Some(envelope);
    });
    match stored {
        Ok(()) => Ok(()),
        Err(error) => {
            let storage = plan_usage_storage_failure(subscription_id, provider, error)
                .caused_by(refused.clone());
            remember_plan_usage_failure(subscription_id, provider, now, &storage);
            Err(storage)
        }
    }
}
