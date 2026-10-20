//! Kimi Code's `/usages` report, read in the shape Moonshot's own client reads
//! it (https://github.com/MoonshotAI/kimi-code/blob/5efca0c3116743855c28426000073bfe34a4862f/packages/oauth/src/managed-usage.ts):
//! `$.usage` holds the plan's `used`, `limit` and `resetTime`; each entry of
//! `$.limits` holds a `window` (`duration` and the proto enum `timeUnit`), a
//! `detail` with the same three counters, and may hold a `name` for its label.
//! Counters arrive as decimal strings. Any other shape is refused by path.

use serde_json::{Map, Value};

use super::super::window::window_label_from_minutes;
use super::claim::{nonnegative_number, optional_instant_ms, positive_number, required_object};
use super::{DAYS_PER_WEEK, MINUTES_PER_DAY, MINUTES_PER_HOUR};
use crate::types::LimitReading;

/// The plan's own quota (`$.usage`, which may be null) and every windowed
/// limit (`$.limits`, which may be empty).
pub(super) fn kimi_usage_readings(
    body: &Value,
    recorded_at_ms: i64,
) -> Result<Vec<LimitReading>, String> {
    let root = required_object(Some(body), "$")?;
    let usage = root
        .get("usage")
        .ok_or_else(|| "missing `$.usage`".to_string())?;
    let limits = root
        .get("limits")
        .ok_or_else(|| "missing `$.limits`".to_string())?
        .as_array()
        .ok_or_else(|| "`$.limits` must be an array".to_string())?;
    let mut readings = Vec::new();
    if !usage.is_null() {
        let counters = required_object(Some(usage), "$.usage")?;
        readings.push(reading(
            "kimi:usage".to_string(),
            "Kimi plan quota".to_string(),
            None,
            counters,
            "$.usage",
            recorded_at_ms,
        )?);
    }
    for (index, entry) in limits.iter().enumerate() {
        let path = format!("$.limits[{index}]");
        let entry = required_object(Some(entry), &path)?;
        let window_path = format!("{path}.window");
        let window = window_label(
            required_object(entry.get("window"), &window_path)?,
            &window_path,
        )?;
        let label = match entry.get("name") {
            None | Some(Value::Null) => format!("Kimi {window} quota"),
            Some(Value::String(name)) if !name.trim().is_empty() => name.trim().to_string(),
            Some(_) => return Err(format!("`{path}.name` must be a non-empty string")),
        };
        let detail_path = format!("{path}.detail");
        let counters = required_object(entry.get("detail"), &detail_path)?;
        // The position is the id because the report names its windows in prose:
        // keying on that prose would turn a reworded label into a second row.
        readings.push(reading(
            format!("kimi:limit-{index}"),
            label,
            Some(window),
            counters,
            &detail_path,
            recorded_at_ms,
        )?);
    }
    Ok(readings)
}

/// One quota from its `used`, `limit` and optional `resetTime`.
fn reading(
    limit_id: String,
    label: String,
    window_label: Option<String>,
    counters: &Map<String, Value>,
    path: &str,
    recorded_at_ms: i64,
) -> Result<LimitReading, String> {
    let limit = positive_number(counters.get("limit"), &format!("{path}.limit"))?;
    let used = nonnegative_number(counters.get("used"), &format!("{path}.used"))?;
    if used > limit {
        return Err(format!("`{path}.used` exceeds `{path}.limit`"));
    }
    Ok(LimitReading {
        limit_id,
        label,
        window_label,
        used_fraction: used / limit,
        resets_at_ms: optional_instant_ms(counters.get("resetTime"), &format!("{path}.resetTime"))?,
        recorded_at_ms: Some(recorded_at_ms),
    })
}

/// The window's length as a label, from its `duration` and its `timeUnit`.
fn window_label(window: &Map<String, Value>, path: &str) -> Result<String, String> {
    let duration = positive_number(window.get("duration"), &format!("{path}.duration"))?;
    let unit = window
        .get("timeUnit")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("`{path}.timeUnit` must be a string"))?;
    let minutes = match unit {
        "TIME_UNIT_MINUTE" => duration,
        "TIME_UNIT_HOUR" => duration * MINUTES_PER_HOUR,
        "TIME_UNIT_DAY" => duration * MINUTES_PER_DAY,
        "TIME_UNIT_WEEK" => duration * DAYS_PER_WEEK * MINUTES_PER_DAY,
        other => {
            return Err(format!(
                "`{path}.timeUnit` is `{other}`, which Kimi does not define"
            ));
        }
    };
    if !minutes.is_finite() || minutes >= i64::MAX as f64 {
        return Err(format!("`{path}.duration` is outside the supported range"));
    }
    Ok(window_label_from_minutes(minutes))
}
