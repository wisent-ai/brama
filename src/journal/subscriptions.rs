//! The journal's records of what was done to a subscription: operator
//! refreshes, sign-ins and account acquisitions, and the reads of the newest
//! of each.

use serde_json::{json, Value};

use super::{append, field, now, read_all};

/// Record one operator-run credential refresh, with the reason they gave.
///
/// A refresh mutates state every later request depends on: the provider
/// invalidates the previous refresh token the moment it issues a new one, so a
/// grant rotated by hand is a grant nothing else can go back to. The reason is
/// stored beside the verdict because the question after a pool recovers -- or
/// stays empty -- is who asked for this and what the product told them, and
/// neither the provider's logs nor the ledger can answer that.
///
/// Every attempt is recorded, including one that found nothing to refresh. No
/// credential material is written here, exactly as everywhere else in this file.
pub fn record_subscription_refresh(
    provider: &str,
    reason: &str,
    result: &str,
    attempted: usize,
    detail: &str,
) -> Result<(), String> {
    append(json!({
        "kind": "subscription_refresh",
        "provider": provider,
        "reason": reason,
        "result": result,
        "attempted": attempted,
        "detail": detail,
        "at": now(),
    }))
}

/// Retain the exact non-secret authentication outcome, including its source
/// revision, failed stage and Weles run id. Timestamps alone cannot say what ran.
pub fn record_subscription_sign_in(outcome: &Value) -> Result<(), String> {
    let mut record = outcome.clone();
    record["kind"] = json!("subscription_sign_in");
    record["at"] = json!(now());
    record["at_ms"] = json!(chrono::Utc::now().timestamp_millis());
    append(record)
}

/// The newest completed automatic or operator sign-in for one subscription.
pub fn latest_subscription_sign_in(subscription_id: &str) -> Result<Option<Value>, String> {
    Ok(read_all()?.into_iter().rev().find(|record| {
        field(record, "kind") == Some("subscription_sign_in")
            && field(record, "subscription_id") == Some(subscription_id)
    }))
}

/// When the newest completed sign-in for one subscription happened, or `None`
/// when none ever has. A sign-in record without its `at_ms` is refused with
/// the record: read as the epoch, it made every sign-in look long past its
/// cooldown.
///
/// Distinct from "some time ago": a caller deciding whether another browser
/// sign-in could produce a different answer needs the instant, not an elapsed
/// window, because what it compares against is when the stored credential's
/// verdict was recorded.
pub fn latest_subscription_sign_in_at_ms(subscription_id: &str) -> Result<Option<i64>, String> {
    let Some(latest) = latest_subscription_sign_in(subscription_id)? else {
        return Ok(None);
    };
    match latest.get("at_ms").and_then(Value::as_i64) {
        Some(at_ms) => Ok(Some(at_ms)),
        None => Err(format!(
            "the journal's newest sign-in for {subscription_id} carries no at_ms: {latest}"
        )),
    }
}

/// Whether another browser sign-in may start after the persisted cooldown.
///
/// The journal, not process memory, is authoritative so restarting Brama cannot
/// turn one refusal into repeated Google sign-in and 2FA prompts.
pub fn subscription_sign_in_due(
    subscription_id: &str,
    cooldown: std::time::Duration,
) -> Result<bool, String> {
    let Some(at_ms) = latest_subscription_sign_in_at_ms(subscription_id)? else {
        return Ok(true);
    };
    let cooldown_ms = i64::try_from(cooldown.as_millis()).map_err(|_| {
        format!(
            "the sign-in cooldown {cooldown:?} for {subscription_id} is longer than milliseconds \
             since the epoch can be compared with"
        )
    })?;
    Ok(chrono::Utc::now().timestamp_millis().saturating_sub(at_ms) >= cooldown_ms)
}

/// Retain one account acquisition: the accounts counted, the plan read, the
/// Weles run and its stages, and how it ended. Written when the purchase is
/// requested (`requested`) and again when it ends, so a purchase whose answer
/// never came back is still on record as asked for.
pub fn record_subscription_acquisition(outcome: &Value) -> Result<(), String> {
    let mut record = outcome.clone();
    record["kind"] = json!("subscription_acquisition");
    record["at"] = json!(now());
    record["at_ms"] = json!(chrono::Utc::now().timestamp_millis());
    append(record)
}

/// The newest acquisition record for one provider, of any result.
pub fn latest_subscription_acquisition(provider: &str) -> Result<Option<Value>, String> {
    Ok(read_all()?.into_iter().rev().find(|record| {
        field(record, "kind") == Some("subscription_acquisition")
            && field(record, "provider") == Some(provider)
    }))
}
