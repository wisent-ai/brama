//! Choose a usable reset from a fresh provider offer and the declared policy.

use super::declaration::ResetDeclaration;
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
    let mut latest = None;
    for reading in exhausted {
        let until = reading.resets_at_ms.ok_or_else(|| {
            format!(
                "subscription {member}: exhausted window {} has no natural reset time",
                reading.limit_id
            )
        })?;
        latest = Some(latest.map_or(until, |previous: i64| previous.max(until)));
    }
    Ok(latest)
}

fn number(key: Option<&str>, field: &str) -> Result<u64, String> {
    let key = key.ok_or_else(|| format!("reset policy does not declare {field}"))?;
    super::super::declaration::stated(key)
}

pub fn choose<'a>(
    offer: &'a ResetOffer,
    declared: &ResetDeclaration,
    blocked_until_ms: Option<i64>,
    automatic: bool,
) -> Result<Option<(&'a ResetCredit, &'static str)>, String> {
    if automatic && !declared.auto_redeem {
        return Ok(None);
    }
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
    let keep = number(declared.keep_credits.as_deref(), "keep_credits")?;
    let minutes = i64::try_from(number(
        declared.min_blocked_minutes.as_deref(),
        "min_blocked_minutes",
    )?)
    .map_err(|error| format!("reset blocked-minute declaration: {error}"))?;
    let hours = i64::try_from(number(
        declared.salvage_horizon_hours.as_deref(),
        "salvage_horizon_hours",
    )?)
    .map_err(|error| format!("reset salvage-hour declaration: {error}"))?;
    let minimum = chrono::TimeDelta::try_minutes(minutes)
        .ok_or_else(|| "reset blocked-minute declaration exceeds supported duration".to_owned())?;
    let salvage = chrono::TimeDelta::try_hours(hours)
        .ok_or_else(|| "reset salvage-hour declaration exceeds supported duration".to_owned())?;
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
