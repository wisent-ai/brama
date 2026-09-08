//! What an operator's paid completion probe records about one subscription.
//!
//! This is the only check that spends the thing it measures, so it never runs
//! on a timer and the verdict it writes is handed straight back to whoever
//! asked for it. It answers the one question a free usage report cannot:
//! whether the provider will actually serve this credential.
//!
//! Any plan windows the probe's answer carried are recorded by the ordinary
//! spend path, which is where every provider answer's headers land. This file
//! adds only the verdict, and only to the field completions own.

use crate::subscription_dispatch::usage::{now_ms, with_ledger, REASON_LIMIT};

use super::{CheckSource, Probe};

/// Trim one stored sentence to the bound every reason in this file shares.
fn bounded_reason(detail: Option<&str>) -> Option<String> {
    detail
        .map(str::trim)
        .filter(|detail| !detail.is_empty())
        .map(|detail| detail.chars().take(REASON_LIMIT).collect::<String>())
}

/// Record what an operator's on-demand completion probe learned about one
/// subscription.
///
/// The probe answers the one question a free usage report cannot: whether the
/// provider will actually serve this credential. It costs a completion, so it
/// runs only when somebody asks, and the verdict it returns is handed straight
/// back to whoever asked. Any readings the probe's answer carried are recorded by
/// [`record_call_from`](crate::subscription_dispatch::usage::record_call_from) on
/// the same path real traffic takes; this only adds the verdict.
pub fn record_probe(
    subscription_id: &str,
    provider: &str,
    ok: bool,
    detail: Option<&str>,
) -> Probe {
    let now = now_ms();
    let probe = Probe {
        attempted_at_ms: now,
        ok,
        detail: bounded_reason(detail),
        source: Some(CheckSource::Completion),
    };
    let recorded = probe.clone();
    with_ledger(|ledger| {
        let entry = ledger
            .subscriptions
            .entry(subscription_id.to_string())
            .or_default();
        entry.provider = provider.to_string();
        entry.updated_at_ms = Some(now);
        entry.probe = Some(probe);
    });
    recorded
}
