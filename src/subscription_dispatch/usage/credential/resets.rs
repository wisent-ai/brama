//! Reset state lives beside usage; a failed read keeps the last provider offer.

use super::super::{now_ms, write_ledger};
use crate::subscription_dispatch::acquire::resets::model::{
    ResetObservation, ResetOffer, ResetRedemption,
};

pub fn record_reset_offer(
    id: &str,
    provider: &str,
    outcome: Result<ResetOffer, String>,
) -> Result<(), String> {
    let (_, stored) = write_ledger(|ledger| {
        let entry = ledger.subscriptions.entry(id.to_owned()).or_default();
        entry.provider = provider.to_owned();
        let (offer, error) = match outcome {
            Ok(offer) => (Some(offer), None),
            Err(error) => (
                entry
                    .resets
                    .as_ref()
                    .and_then(|previous| previous.offer.clone()),
                Some(error),
            ),
        };
        entry.resets = Some(ResetObservation {
            offer,
            error,
            attempted_at_ms: now_ms(),
        });
        entry.updated_at_ms = Some(now_ms());
    });
    stored
}

pub fn record_reset_redemption(outcome: ResetRedemption) -> Result<(), String> {
    let (_, stored) = write_ledger(|ledger| {
        let entry = ledger
            .subscriptions
            .entry(outcome.member.clone())
            .or_default();
        entry.provider = outcome.provider.clone();
        entry.updated_at_ms = Some(now_ms());
        entry.reset_redemption = Some(outcome);
    });
    stored
}

/// Called only after a confirmed reset and fresh usage reports with no exhausted window.
pub fn record_reset_unblocked(id: &str, provider: &str) -> Result<(), String> {
    let (_, stored) = write_ledger(|ledger| {
        if let Some(entry) = ledger.subscriptions.get_mut(id) {
            if entry.provider == provider
                && entry
                    .block
                    .as_ref()
                    .is_some_and(|block| !block.quota_exhausted)
            {
                entry.block = None;
                entry.updated_at_ms = Some(now_ms());
            }
        }
    });
    stored
}
