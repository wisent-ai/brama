//! Which of a subscription's plan windows a reader is shown, where each came
//! from, and how full the tightest one is.
//!
//! A plan window is the provider's own statement about its own quota: what
//! fraction of a ration is gone and when the ration resets. Brama never
//! computes one, so everything here is a projection of readings somebody else
//! wrote -- which of them are still worth serving, which of the three sources
//! they came from, whether the newest of them can honestly be called current,
//! and the one number the router places candidates by.
//!
//! Serving a stale reading is deliberate and is the reason this projection
//! exists at all: a reading that says when it was taken is information, an
//! empty plan is not, and the difference between the two is a row that goes
//! blank because an upstream had a bad ten minutes. How long that stays true
//! lives in `freshness`.

mod freshness;

use serde::{Deserialize, Serialize};

use crate::types::LimitReading;

use super::{now_ms, read_ledger, usage_for, SubscriptionUsage};

pub use freshness::{jittered_plan_usage_ttl_ms, plan_usage_retention_ms, plan_usage_ttl_ms};

/// Where one plan reading came from.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum UsageSource {
    /// The provider's own usage report, read without spending any quota. The
    /// strongest of the three: it is the vendor's current statement about the
    /// account, asked for on purpose.
    Provider,
    /// The headers of a request a caller actually made. Authoritative when it
    /// arrives and silent otherwise, which is why it cannot be the only source.
    Traffic,
    /// An operator's on-demand probe, which spends one minimal completion.
    Probe,
}

impl UsageSource {
    /// The stored name, which is also the name every reader sees.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Provider => "provider",
            Self::Traffic => "traffic",
            Self::Probe => "probe",
        }
    }
}

/// Whether this subscription's usage report is due to be read again.
///
/// A recent report attempt owns the cache window, whether it succeeded or
/// failed. Otherwise every retained window must still be current; one new
/// traffic header cannot make an older or already-reset window fresh.
pub fn plan_usage_due(subscription_id: &str) -> bool {
    let now = now_ms();
    let window = jittered_plan_usage_ttl_ms(subscription_id);
    read_ledger(|ledger| {
        let Some(entry) = ledger.subscriptions.get(subscription_id) else {
            return true;
        };
        let checked_recently = entry
            .plan_usage_checked_at_ms
            .is_some_and(|checked| now.saturating_sub(checked) < window);
        let readings_current = !entry.limits.is_empty()
            && entry.limits.values().all(|reading| {
                let recorded_is_current = reading.recorded_at_ms > 0
                    && now.saturating_sub(reading.recorded_at_ms) < window;
                let reset_is_current = reading
                    .resets_at_ms
                    .is_none_or(|resets_at_ms| resets_at_ms > now);
                recorded_is_current && reset_is_current
            });
        !checked_recently && !readings_current
    })
}

/// The plan windows a reader should be shown, where they came from, and whether
/// any retained window or the newest usage-report attempt is stale.
pub struct PlanWindows {
    pub limits: Vec<LimitReading>,
    pub source: Option<UsageSource>,
    pub stale: bool,
}

/// Project one subscription's readings for a reader.
///
/// A stale reading is served, with `stale` set, because a reading that states
/// when it was taken is information and an empty plan is not -- an upstream that
/// fails for ten minutes must not blank a row that was right ten minutes ago.
/// Past the retention window it stops being served: a fraction of a five-hour
/// window that has since reset four times describes nothing, and a reader has no
/// way to know that from the number alone.
pub fn plan_windows(usage: Option<&SubscriptionUsage>) -> PlanWindows {
    let Some(usage) = usage else {
        return PlanWindows {
            limits: Vec::new(),
            source: None,
            stale: false,
        };
    };
    let now = now_ms();
    let retention = plan_usage_retention_ms();
    let limits = usage
        .limits
        .values()
        .filter(|reading| {
            // A reading with no instant is still useful legacy evidence, but
            // cannot truthfully be called current; the freshness projection
            // below marks it stale.
            reading.recorded_at_ms == 0 || now.saturating_sub(reading.recorded_at_ms) <= retention
        })
        .cloned()
        .collect::<Vec<_>>();
    if limits.is_empty() {
        return PlanWindows {
            limits,
            source: None,
            stale: false,
        };
    }
    let stale = usage.usage_check.as_ref().is_some_and(|check| !check.ok)
        || limits.iter().any(|reading| {
            (reading.recorded_at_ms == 0
                || now.saturating_sub(reading.recorded_at_ms) > plan_usage_ttl_ms())
                || reading
                    .resets_at_ms
                    .is_some_and(|resets_at_ms| resets_at_ms <= now)
        });
    PlanWindows {
        stale,
        limits,
        source: usage.usage_source,
    }
}

/// The earliest future reset instant across this subscription's served windows.
///
/// A pin on this credential should die with its tightest window, and this is
/// that window's end. `None` means no served reading names a future reset, so
/// the caller falls back to its own default rather than treating the pin as
/// immortal.
pub fn next_reset_ms(subscription_id: &str) -> Option<i64> {
    let entry = usage_for(subscription_id)?;
    let windows = plan_windows(Some(&entry));
    let now = now_ms();
    windows
        .limits
        .iter()
        .filter_map(|reading| reading.resets_at_ms)
        .filter(|resets_at_ms| *resets_at_ms > now)
        .min()
}

/// How much of this subscription's tightest current plan window is spent.
///
/// This is the routing view of the ledger: the maximum used fraction across
/// the windows [`plan_windows`] still serves, with one adjustment -- a window
/// whose own reset instant has passed counts as empty, because the provider's
/// clock says it rolled and charging its last reading against the account
/// would freeze a credential that is free again. Readings past the retention
/// window are already absent from the projection, so they cannot route
/// anything either.
///
/// `None` means nothing usable is recorded, which is not the same statement as
/// a known-empty plan: it covers a subscription no traffic ever reached and a
/// provider that publishes no windows at all. Callers placing candidates treat
/// it as fully available, because the first real call writes the reading that
/// corrects the placement.
pub fn used_fraction(subscription_id: &str) -> Option<f64> {
    let entry = usage_for(subscription_id)?;
    let windows = plan_windows(Some(&entry));
    if windows.limits.is_empty() {
        return None;
    }
    let now = now_ms();
    Some(
        windows
            .limits
            .iter()
            .map(|reading| match reading.resets_at_ms {
                Some(resets_at_ms) if resets_at_ms <= now => 0.0,
                _ => reading.used_fraction,
            })
            .fold(0.0_f64, f64::max),
    )
}
