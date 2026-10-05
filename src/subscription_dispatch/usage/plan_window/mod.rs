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
//! Serving a reading after its source started failing is deliberate and is
//! the reason this projection exists at all: a reading that says when it was
//! taken is information, an empty plan is not.

use serde::{Deserialize, Serialize};

use crate::types::LimitReading;

use super::{now_ms, read_ledger, usage_for, SubscriptionUsage};

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
/// A reading is current until the window it describes resets, which is the
/// provider's own statement. A subscription is due when it has no reading, a
/// reading without a reset instant (it cannot say how long it holds, so every
/// maintenance pass the operator's schedule runs reads it again), a reading
/// whose window has reset, or a last report attempt that failed. How often
/// passes run is the host's Stado schedule; no cache window is chosen here.
pub fn plan_usage_due(subscription_id: &str) -> bool {
    let now = now_ms();
    read_ledger(|ledger| {
        let Some(entry) = ledger.subscriptions.get(subscription_id) else {
            return true;
        };
        let last_check_failed = entry.usage_check.as_ref().is_some_and(|check| !check.ok);
        last_check_failed
            || entry.limits.is_empty()
            || entry.limits.values().any(|reading| {
                reading
                    .resets_at_ms
                    .is_none_or(|resets_at_ms| resets_at_ms <= now)
            })
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
/// Every recorded reading is served: a reading that states when it was taken
/// and when its window resets is information, and an empty plan is not. A
/// reading whose window has reset is still shown (the router counts it as
/// empty) until a newer reading replaces it. `stale` says the newest usage
/// report failed or a served window has already reset.
pub fn plan_windows(usage: Option<&SubscriptionUsage>) -> PlanWindows {
    let Some(usage) = usage else {
        return PlanWindows {
            limits: Vec::new(),
            source: None,
            stale: false,
        };
    };
    let now = now_ms();
    let limits = usage.limits.values().cloned().collect::<Vec<_>>();
    if limits.is_empty() {
        return PlanWindows {
            limits,
            source: None,
            stale: false,
        };
    }
    let stale = usage.usage_check.as_ref().is_some_and(|check| !check.ok)
        || limits.iter().any(|reading| {
            reading.recorded_at_ms == 0
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
