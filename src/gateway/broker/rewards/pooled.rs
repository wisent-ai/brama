//! The contribution a donated subscription makes: serving the shared pool.
//!
//! Each calendar day (UTC) on which a donated subscription answers a caller
//! from the pool is one `subscription.pooled` day for the agent that banked
//! it. The event id is the subscription and the day, so the ledger records a
//! day once however many requests it served and however often the gateway
//! restarted; this process asks once per day and subscription it has not
//! already reported.

use std::collections::HashSet;
use std::sync::{LazyLock, Mutex};

use serde_json::{json, Value};

use super::ledger::{ledger_url, report, Unanswered};
use super::PRODUCT;

/// The days this process has reported, as (subscription, day).
static REPORTED: LazyLock<Mutex<HashSet<(String, String)>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));

/// One served day: the unit `subscription.pooled` is counted in.
const ONE_DAY: u64 = std::num::NonZeroU64::MIN.get();

/// The agent that banked donated subscription `id`, from the donation overlay;
/// `None` when the subscription was not donated.
fn donor_of(id: &str) -> Result<Option<String>, String> {
    let path = super::super::subscription::donated_subscriptions_path()?;
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(format!(
                "read donated subscriptions file `{}`: {error}",
                path.display()
            ))
        }
    };
    let overlay: Value = serde_json::from_str(&text).map_err(|error| {
        format!(
            "decode donated subscriptions file `{}`: {error}",
            path.display()
        )
    })?;
    let items = overlay
        .get("items")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            format!(
                "donated subscriptions file `{}` has no items list",
                path.display()
            )
        })?;
    Ok(items
        .iter()
        .find(|item| item.get("id").and_then(Value::as_str) == Some(id))
        .and_then(|item| item.get("agent_id").and_then(Value::as_str))
        .map(str::to_string))
}

/// A pooled call to `subscription_id` succeeded: report the day it served,
/// once, in the background. A failure is logged with its cause and the day is
/// asked again on the next call that day, never counted as reported.
pub(crate) fn note_served(subscription_id: &str, provider: &str) {
    match ledger_url() {
        Ok(Some(_)) => {}
        // No ledger is declared: this gateway reports nothing.
        Ok(None) => return,
        Err(error) => {
            tracing::warn!(event = "contribution_report_failed", subscription_id, %error,
                "the contribution ledger's declaration could not be read");
            return;
        }
    }
    let day = chrono::Utc::now().date_naive().to_string();
    let key = (subscription_id.to_string(), day.clone());
    // One insert per reported day: a holder that panicked left the set whole,
    // so it is taken back (and logged) rather than re-reporting every day of
    // every pooled subscription for the life of the process.
    if crate::core::sync::held(&REPORTED, "reported pool days").contains(&key) {
        return;
    }
    let provider = provider.to_string();
    let runtime = match tokio::runtime::Handle::try_current() {
        Ok(runtime) => runtime,
        Err(error) => {
            tracing::warn!(event = "contribution_report_failed", subscription_id, %error,
                "a pooled subscription day was served outside the gateway's runtime and is not reported");
            return;
        }
    };
    runtime.spawn(async move {
        let (subscription_id, day) = key.clone();
        let donor = match donor_of(&subscription_id).and_then(|donor| donor.map(|agent| super::contributor_of(&agent)).transpose()) {
            Ok(Some(donor)) => donor,
            Ok(None) => return,
            Err(error) => {
                tracing::warn!(event = "contribution_report_failed", subscription_id, %error,
                    "the donor of a pooled subscription could not be read");
                return;
            }
        };
        let event = json!({
            "source_event_id": format!("{subscription_id}:{day}"),
            "kind": "subscription.pooled",
            "contributor_id": donor,
            "quantity": ONE_DAY,
            "unit": "day",
            "evidence": {"subscription_id": subscription_id, "provider": provider, "day": day, "reporter": PRODUCT},
            "occurred_at": chrono::Utc::now().to_rfc3339(),
        });
        match report(&event).await {
            Ok(_) => {
                crate::core::sync::held(&REPORTED, "reported pool days").insert(key);
            }
            Err(Unanswered::Undeclared) => {
                // No ledger is declared: nothing is reported, and nothing is
                // remembered, so declaring one reports the day's next call.
            }
            Err(Unanswered::Failed(cause)) => {
                tracing::warn!(event = "contribution_report_failed", subscription_id, day, %cause,
                    "a pooled subscription day could not be reported to the contribution ledger");
            }
        }
    });
}
