//! Normalize evaluated Cedar and Juniper offers without guessing absent reports.

use crate::subscription_dispatch::acquire::resets::model::{ResetCredit, ResetOffer};
use serde::Deserialize;
use serde_json::Value;

#[derive(Deserialize)]
struct Cedar {
    eligible: bool,
    ineligible_reason: Option<String>,
    next_grant_id: Option<String>,
    cooldown_until: Option<String>,
    #[serde(default)]
    grants: Vec<Grant>,
}

#[derive(Deserialize)]
struct Grant {
    id: String,
    resets_left: u64,
    starts_at: Option<String>,
    ends_at: Option<String>,
    usable_now: Option<bool>,
    paused: Option<bool>,
    use_requires_limit: Option<bool>,
    #[serde(default)]
    clears: Vec<String>,
}

#[derive(Deserialize)]
struct Juniper {
    eligible: bool,
    arm: Option<String>,
    available: Option<bool>,
    ineligible_reason: Option<String>,
    weekly_resets_at: Option<String>,
}

fn instant(value: Option<String>) -> Result<Option<i64>, String> {
    value
        .map(|value| {
            chrono::DateTime::parse_from_rfc3339(&value)
                .map(|date| date.timestamp_millis())
                .map_err(|error| format!("Claude reset expiry {value}: {error}"))
        })
        .transpose()
}

pub(super) fn cedar(body: &Value, now: i64) -> Result<Option<ResetOffer>, String> {
    let Some(value) = body.get("cedar_ember").filter(|value| !value.is_null()) else {
        return Ok(None);
    };
    let status: Cedar = serde_json::from_value(value.clone())
        .map_err(|error| format!("Claude Cedar report: {error}"))?;
    let cooling = instant(status.cooldown_until)?.is_some_and(|until| until > now);
    let mut credits = Vec::with_capacity(status.grants.len());
    for grant in status.grants {
        if grant.id.is_empty() {
            return Err("Claude Cedar report contains an empty grant id".into());
        }
        let expires_at_ms = instant(grant.ends_at)?;
        let starts_at_ms = instant(grant.starts_at)?;
        let usable = status.eligible
            && !cooling
            && grant.paused != Some(true)
            && grant.usable_now == Some(true)
            && starts_at_ms.is_none_or(|start| start <= now)
            && status.next_grant_id.as_deref() == Some(grant.id.as_str())
            && std::num::NonZeroU64::new(grant.resets_left).is_some()
            && expires_at_ms.is_none_or(|expiry| expiry > now);
        credits.push(ResetCredit {
            id: grant.id,
            program: "cedar_ember".into(),
            remaining_count: grant.resets_left,
            consumption_observable: true,
            usable,
            requires_limit: grant.use_requires_limit != Some(false),
            expires_at_ms,
            clears: grant.clears,
        });
    }
    Ok(Some(ResetOffer {
        available_count: credits
            .iter()
            .filter(|credit| credit.expires_at_ms.is_none_or(|expiry| expiry > now))
            .map(|credit| credit.remaining_count)
            .sum(),
        redeemable_count: Some(
            credits
                .iter()
                .filter(|credit| credit.usable)
                .map(|credit| credit.remaining_count)
                .sum(),
        ),
        eligible: Some(status.eligible),
        limit_reached: None,
        reason: status.ineligible_reason,
        next_credit_id: status.next_grant_id,
        credits,
        observed_at_ms: now,
    }))
}

pub(super) fn juniper(body: &Value, now: i64) -> Result<Option<ResetOffer>, String> {
    let Some(value) = body.get("juniper_tide").filter(|value| !value.is_null()) else {
        return Ok(None);
    };
    let status: Juniper = serde_json::from_value(value.clone())
        .map_err(|error| format!("Claude Juniper report: {error}"))?;
    let offered = status.arm.as_deref() == Some("reset");
    let usable = offered && status.eligible && status.available == Some(true);
    let expires_at_ms = instant(status.weekly_resets_at)?;
    let credits = if offered {
        vec![ResetCredit {
            id: "juniper_tide".into(),
            program: "juniper_tide".into(),
            remaining_count: u64::from(status.available == Some(true)),
            consumption_observable: false,
            usable,
            requires_limit: true,
            expires_at_ms,
            clears: vec!["five_hour".into()],
        }]
    } else {
        Vec::new()
    };
    Ok(Some(ResetOffer {
        available_count: u64::from(usable),
        redeemable_count: Some(u64::from(usable)),
        eligible: Some(status.eligible),
        limit_reached: None,
        reason: status.ineligible_reason,
        next_credit_id: usable.then(|| "juniper_tide".into()),
        credits,
        observed_at_ms: now,
    }))
}

pub(super) fn combine(
    cedar: Option<ResetOffer>,
    juniper: Option<ResetOffer>,
) -> Result<ResetOffer, String> {
    match (cedar, juniper) {
        (None, None) => {
            Err("Claude evaluated neither reset program; availability remains unknown".into())
        }
        (Some(offer), None) | (None, Some(offer)) => Ok(offer),
        (Some(mut cedar), Some(juniper)) => {
            cedar.available_count = cedar
                .available_count
                .checked_add(juniper.available_count)
                .ok_or_else(|| {
                    "Claude aggregate reset count exceeds the supported integer range".to_owned()
                })?;
            cedar.redeemable_count = match (cedar.redeemable_count, juniper.redeemable_count) {
                (Some(left), Some(right)) => Some(left.checked_add(right).ok_or_else(|| {
                    "Claude aggregate redeemable count exceeds the supported integer range"
                        .to_owned()
                })?),
                _ => None,
            };
            cedar.eligible = match (cedar.eligible, juniper.eligible) {
                (Some(true), _) | (_, Some(true)) => Some(true),
                (Some(false), Some(false)) => Some(false),
                _ => None,
            };
            cedar.next_credit_id = None;
            cedar.credits.extend(juniper.credits);
            cedar.reason = match (cedar.reason, juniper.reason) {
                (Some(left), Some(right)) => Some(format!("Cedar: {left}; Juniper: {right}")),
                (left, right) => left.or(right),
            };
            Ok(cedar)
        }
    }
}
