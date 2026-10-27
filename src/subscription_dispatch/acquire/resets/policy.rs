//! Choose a usable reset from a fresh provider offer and the operator's
//! stated policy.

use super::model::{ResetCredit, ResetOffer};
use crate::subscription_dispatch::{plan_usage, usage};

pub async fn blocked_until(member: &str, provider: &str) -> Result<Option<i64>, String> {
    plan_usage::refresh(member, provider)
        .await
        .map_err(|error| error.to_json())?;
    let entry = usage::usage_for(member);
    if entry
        .as_ref()
        .and_then(|entry| entry.resets.as_ref())
        .and_then(|reading| reading.offer.as_ref())
        .and_then(|offer| offer.limit_reached)
        == Some(false)
    {
        return Ok(None);
    }
    let windows = usage::plan_windows(entry.as_ref());
    if windows.stale || windows.limits.is_empty() {
        return Err(format!(
            "subscription {member}: no fresh provider windows to decide reset eligibility"
        ));
    }
    let now = chrono::Utc::now().timestamp_millis();
    let mut exhausted = windows
        .limits
        .iter()
        .filter(|reading| {
            reading.resets_at_ms.is_none_or(|until| until > now)
                && super::super::decide::pool_state::at_limit(reading.used_fraction)
        })
        .peekable();
    if exhausted.peek().is_none() {
        return Ok(None);
    }
    let mut latest: Option<i64> = None;
    for reading in exhausted {
        let until = reading.resets_at_ms.ok_or_else(|| {
            format!(
                "subscription {member}: exhausted window {} has no natural reset time",
                reading.limit_id
            )
        })?;
        latest = Some(match latest {
            Some(previous) => previous.max(until),
            None => until,
        });
    }
    Ok(latest)
}

/// The credit to redeem from `offer`: for a reset the operator asked for, the
/// one expiring first; automatically, only under the policy he stated for
/// `provider` (`settings.rs`), refused by name when he stated none.
pub fn choose<'a>(
    provider: &str,
    offer: &'a ResetOffer,
    blocked_until_ms: Option<i64>,
    automatic: bool,
) -> Result<Option<(&'a ResetCredit, &'static str)>, String> {
    let now = chrono::Utc::now().timestamp_millis();
    let mut credits: Vec<_> = offer
        .credits
        .iter()
        .filter(|credit| {
            credit.usable
                && std::num::NonZeroU64::new(credit.remaining_count).is_some()
                && credit.expires_at_ms.is_none_or(|expiry| expiry > now)
                && (!credit.requires_limit || blocked_until_ms.is_some())
        })
        .collect();
    credits.sort_by_key(|credit| (credit.expires_at_ms.is_none(), credit.expires_at_ms));
    if !automatic {
        return Ok(credits.first().map(|credit| (*credit, "operator")));
    }
    let stated = super::settings::for_provider(provider)?;
    let keep = stated.keep_credits;
    let minutes = i64::try_from(stated.min_blocked_minutes)
        .map_err(|error| format!("{provider}'s stated blocked minutes: {error}"))?;
    let hours = i64::try_from(stated.salvage_horizon_hours)
        .map_err(|error| format!("{provider}'s stated salvage hours: {error}"))?;
    let minimum = chrono::TimeDelta::try_minutes(minutes).ok_or_else(|| {
        format!("{provider}'s stated blocked minutes exceed a supported duration")
    })?;
    let salvage = chrono::TimeDelta::try_hours(hours)
        .ok_or_else(|| format!("{provider}'s stated salvage hours exceed a supported duration"))?;
    for credit in credits {
        if credit
            .expires_at_ms
            .is_some_and(|expiry| expiry.saturating_sub(now) <= salvage.num_milliseconds())
        {
            return Ok(Some((credit, "expiring_credit")));
        }
        if offer.available_count > keep
            && blocked_until_ms
                .is_some_and(|until| until.saturating_sub(now) > minimum.num_milliseconds())
        {
            return Ok(Some((credit, "exhausted_window")));
        }
    }
    Ok(None)
}
