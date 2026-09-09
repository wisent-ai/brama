//! How a usage report document becomes ledger rows.

mod claim;
mod kimi_usage;

use serde_json::Value;

use super::endpoint::PlanUsageShape;
use super::window::window_label_from_minutes;
use crate::types::LimitReading;
use claim::{nonnegative_number, percentage, positive_number};
use claim::{delayed_reset_ms, optional_instant_ms, required_object};
use kimi_usage::kimi_usage_readings;

/// The windows Anthropic's usage report names, mapped onto the very limit ids
/// its response headers produce, so one account's five-hour window stays one row
/// however it was read. The final flag is true only for plan-specific windows
/// Anthropic is documented to omit for accounts that do not have them.
const ANTHROPIC_USAGE_WINDOWS: &[(&str, &str, &str, &str, bool)] = &[
    (
        "five_hour",
        "anthropic:5h",
        "Claude 5 hour",
        "5 hours",
        false,
    ),
    ("seven_day", "anthropic:7d", "Claude 7 day", "7 days", false),
    (
        "seven_day_opus",
        "anthropic:7d-opus",
        "Claude 7 day (Opus)",
        "7 days",
        true,
    ),
    (
        "seven_day_sonnet",
        "anthropic:7d-sonnet",
        "Claude 7 day (Sonnet)",
        "7 days",
        true,
    ),
];

/// The usage reports state utilization as a percentage, while the ledger stores
/// it as a fraction.
const PERCENT_SCALE: f64 = 100.0;
const MS_PER_SECOND: f64 = 1_000.0;
const SECONDS_PER_MINUTE: f64 = 60.0;
const MINUTES_PER_HOUR: f64 = 60.0;
const MINUTES_PER_DAY: f64 = 24.0 * MINUTES_PER_HOUR;
const DAYS_PER_WEEK: f64 = 7.0;
/// Above this an epoch value can only be milliseconds: epoch seconds do not
/// reach it for another thirty thousand years.
const EPOCH_MILLISECONDS_FLOOR: f64 = 1e12;

/// Turn one provider's usage report into validated limit readings.
pub(super) fn plan_usage_readings(
    shape: PlanUsageShape,
    body: &Value,
    recorded_at_ms: i64,
) -> Result<Vec<LimitReading>, String> {
    match shape {
        PlanUsageShape::AnthropicOauth => {
            let root = required_object(Some(body), "$")?;
            let mut readings = Vec::new();
            for (field, limit_id, label, window_label, optional) in ANTHROPIC_USAGE_WINDOWS {
                let Some(value) = root.get(*field) else {
                    if *optional {
                        continue;
                    }
                    return Err(format!("missing `$.{field}`"));
                };
                // Anthropic uses null to explicitly say a plan has no such
                // window. That is distinct from a malformed object.
                if value.is_null() {
                    continue;
                }
                let window = required_object(Some(value), &format!("$.{field}"))?;
                let used =
                    percentage(window.get("utilization"), &format!("$.{field}.utilization"))?;
                if !window.contains_key("resets_at") {
                    return Err(format!("missing `$.{field}.resets_at`"));
                }
                let resets_at_ms =
                    optional_instant_ms(window.get("resets_at"), &format!("$.{field}.resets_at"))?;
                readings.push(LimitReading {
                    limit_id: (*limit_id).to_string(),
                    label: (*label).to_string(),
                    window_label: Some((*window_label).to_string()),
                    used_fraction: used,
                    resets_at_ms,
                    recorded_at_ms,
                });
            }
            Ok(readings)
        }
        PlanUsageShape::CodexWham => {
            let root = required_object(Some(body), "$")?;
            let rate_limit = required_object(root.get("rate_limit"), "$.rate_limit")?;
            let mut readings = Vec::new();
            for (meter, optional) in [("primary", false), ("secondary", true)] {
                let field = format!("{meter}_window");
                let path = format!("$.rate_limit.{field}");
                let Some(value) = rate_limit.get(&field) else {
                    if optional {
                        continue;
                    }
                    return Err(format!("missing `{path}`"));
                };
                if value.is_null() {
                    if optional {
                        continue;
                    }
                    return Err(format!("`{path}` must be an object"));
                }
                let window = required_object(Some(value), &path)?;
                let used = percentage(window.get("used_percent"), &format!("{path}.used_percent"))?;
                let seconds = positive_number(
                    window.get("limit_window_seconds"),
                    &format!("{path}.limit_window_seconds"),
                )?;
                let absolute_reset =
                    optional_instant_ms(window.get("reset_at"), &format!("{path}.reset_at"))?;
                let delayed_reset = match window.get("reset_after_seconds") {
                    None | Some(Value::Null) => None,
                    value => {
                        let delay =
                            nonnegative_number(value, &format!("{path}.reset_after_seconds"))?;
                        Some(delayed_reset_ms(
                            delay,
                            recorded_at_ms,
                            &format!("{path}.reset_after_seconds"),
                        )?)
                    }
                };
                let resets_at_ms = absolute_reset.or(delayed_reset).ok_or_else(|| {
                    format!("`{path}` must contain `reset_at` or `reset_after_seconds`")
                })?;
                readings.push(LimitReading {
                    limit_id: format!("codex:{meter}"),
                    label: format!("Codex {meter} window"),
                    window_label: Some(window_label_from_minutes(seconds / SECONDS_PER_MINUTE)),
                    used_fraction: used,
                    resets_at_ms: Some(resets_at_ms),
                    recorded_at_ms,
                });
            }
            Ok(readings)
        }
        PlanUsageShape::KimiUsages => kimi_usage_readings(body, recorded_at_ms),
    }
}
