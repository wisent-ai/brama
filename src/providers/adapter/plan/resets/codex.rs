//! Codex's saved-reset list is a provider report, not a quota estimate.

use crate::subscription_dispatch::acquire::resets::model::{ResetCredit, ResetOffer};
use serde::Deserialize;
use serde_json::Value;

#[derive(Deserialize)]
struct Listing {
    available_count: u64,
    credits: Vec<Credit>,
}

#[derive(Deserialize)]
struct Credit {
    id: String,
    status: Option<String>,
    expires_at: Option<String>,
}

pub(super) fn parse(body: Value, observed_at_ms: i64) -> Result<ResetOffer, String> {
    let list: Listing = serde_json::from_value(body)
        .map_err(|error| format!("Codex saved reset report: {error}"))?;
    let mut credits = Vec::with_capacity(list.credits.len());
    for credit in list.credits {
        if credit.id.is_empty() {
            return Err("Codex saved reset report contains an empty credit id".into());
        }
        let expires_at_ms = match credit.expires_at {
            Some(expiry) => Some(
                chrono::DateTime::parse_from_rfc3339(&expiry)
                    .map_err(|error| format!("Codex credit {} expiry: {error}", credit.id))?
                    .timestamp_millis(),
            ),
            None => None,
        };
        let usable = credit.status.as_deref() == Some("available")
            && expires_at_ms.is_none_or(|expiry| expiry > observed_at_ms);
        credits.push(ResetCredit {
            id: credit.id,
            program: "codex_wham".into(),
            remaining_count: u64::from(credit.status.as_deref() != Some("redeemed")),
            consumption_observable: credit.status.is_some(),
            usable,
            requires_limit: true,
            expires_at_ms,
            clears: Vec::new(),
        });
    }
    Ok(ResetOffer {
        available_count: list.available_count,
        redeemable_count: None,
        eligible: None,
        limit_reached: None,
        reason: None,
        next_credit_id: None,
        credits,
        observed_at_ms,
    })
}
