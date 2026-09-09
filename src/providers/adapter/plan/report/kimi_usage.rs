//! Kimi's report, which states counters instead of percentages and carries its
//! own reset aliases.

use serde_json::{Map, Value};

use super::super::window::window_label_from_minutes;
use super::claim::{
    delayed_reset_ms, nonnegative_number, optional_instant_ms, optional_number, positive_number,
    required_object,
};
use super::{DAYS_PER_WEEK, MINUTES_PER_DAY, MINUTES_PER_HOUR, SECONDS_PER_MINUTE};
use crate::types::LimitReading;

/// Kimi states quotas as counts. Its top-level usage may explicitly be null and
/// its limits array may be empty; every object it does publish must be complete.
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
    if let Some(reading) = kimi_reading(
        usage,
        "kimi:usage",
        "Kimi plan quota",
        "$.usage",
        recorded_at_ms,
    )? {
        readings.push(reading);
    }
    for (index, entry) in limits.iter().enumerate() {
        let path = format!("$.limits[{index}]");
        let object = required_object(Some(entry), &path)?;
        // The position is the id because the report names its windows in prose:
        // keying on that prose would turn a reworded label into a second row.
        let label = ["name", "title", "scope"]
            .into_iter()
            .filter_map(|field| object.get(field).and_then(Value::as_str))
            .map(str::trim)
            .find(|label| !label.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| format!("Kimi quota {}", index + 1));
        let reading = kimi_reading(
            entry,
            &format!("kimi:limit-{index}"),
            &label,
            &path,
            recorded_at_ms,
        )?
        .ok_or_else(|| format!("`{path}` cannot be null"))?;
        readings.push(reading);
    }
    Ok(readings)
}

fn kimi_reading(
    value: &Value,
    limit_id: &str,
    label: &str,
    path: &str,
    recorded_at_ms: i64,
) -> Result<Option<LimitReading>, String> {
    if value.is_null() {
        return Ok(None);
    }
    let object = required_object(Some(value), path)?;
    // Counters sit either directly on the entry or one level under `detail`.
    // A claimed non-object detail is not silently treated as absent.
    let (counters, counters_path) = match object.get("detail") {
        None | Some(Value::Null) => (object, path.to_string()),
        Some(Value::Object(detail)) => (detail, format!("{path}.detail")),
        Some(_) => return Err(format!("`{path}.detail` must be an object")),
    };
    let limit = positive_number(counters.get("limit"), &format!("{counters_path}.limit"))?;
    let claimed_used = optional_number(counters.get("used"), &format!("{counters_path}.used"))?;
    let claimed_remaining = optional_number(
        counters.get("remaining"),
        &format!("{counters_path}.remaining"),
    )?;
    let used = match (claimed_used, claimed_remaining) {
        (None, None) => {
            return Err(format!(
                "`{counters_path}` must contain either `used` or `remaining`"
            ));
        }
        (Some(used), None) => used,
        (None, Some(remaining)) => limit - remaining,
        (Some(used), Some(remaining)) => {
            let derived = limit - remaining;
            let tolerance = f64::EPSILON * limit.abs().max(1.0) * 8.0;
            if (used - derived).abs() > tolerance {
                return Err(format!(
                    "`{counters_path}.used` and `{counters_path}.remaining` disagree with `{counters_path}.limit`"
                ));
            }
            used
        }
    };
    if !(0.0..=limit).contains(&used) {
        return Err(format!(
            "`{counters_path}.used` must be between zero and `{counters_path}.limit`"
        ));
    }
    if let Some(remaining) = claimed_remaining {
        if !(0.0..=limit).contains(&remaining) {
            return Err(format!(
                "`{counters_path}.remaining` must be between zero and `{counters_path}.limit`"
            ));
        }
    }
    let window = match object.get("window") {
        None | Some(Value::Null) => None,
        Some(Value::Object(window)) => Some(window),
        Some(_) => return Err(format!("`{path}.window` must be an object")),
    };
    Ok(Some(LimitReading {
        limit_id: limit_id.to_string(),
        label: label.to_string(),
        window_label: match window {
            Some(window) => kimi_window_label(window, &format!("{path}.window"))?,
            None => None,
        },
        used_fraction: used / limit,
        resets_at_ms: kimi_resets_at_ms(object, window, path, recorded_at_ms)?,
        recorded_at_ms,
    }))
}

/// Read Kimi's optional reset. A present malformed alias is rejected rather
/// than skipped in favor of a later field.
fn kimi_resets_at_ms(
    entry: &Map<String, Value>,
    window: Option<&Map<String, Value>>,
    path: &str,
    recorded_at_ms: i64,
) -> Result<Option<i64>, String> {
    const RESET_INSTANT_FIELDS: &[&str] = &["reset_at", "resetAt", "reset_time", "resetTime"];
    const RESET_DELAY_FIELDS: &[&str] = &["reset_in", "resetIn", "ttl"];
    let mut absolute_reset = None;
    let mut delayed_reset = None;
    for (source, source_path) in [
        (window, format!("{path}.window")),
        (Some(entry), path.to_string()),
    ] {
        let Some(source) = source else {
            continue;
        };
        for field in RESET_INSTANT_FIELDS {
            if let Some(value) = source.get(*field) {
                let parsed = optional_instant_ms(Some(value), &format!("{source_path}.{field}"))?;
                if absolute_reset.is_none() {
                    absolute_reset = parsed;
                }
            }
        }
        for field in RESET_DELAY_FIELDS {
            if let Some(value) = source.get(*field) {
                if value.is_null() {
                    continue;
                }
                let delay = nonnegative_number(Some(value), &format!("{source_path}.{field}"))?;
                let parsed =
                    delayed_reset_ms(delay, recorded_at_ms, &format!("{source_path}.{field}"))?;
                if delayed_reset.is_none() {
                    delayed_reset = Some(parsed);
                }
            }
        }
    }
    Ok(absolute_reset.or(delayed_reset))
}

/// The optional human label for a Kimi window. If either half of a claimed
/// duration is present, both must be valid.
fn kimi_window_label(window: &Map<String, Value>, path: &str) -> Result<Option<String>, String> {
    let duration_value = window.get("duration");
    let read_unit = |value: Option<&Value>, field: &str| -> Result<Option<String>, String> {
        let Some(value) = value else {
            return Ok(None);
        };
        let unit = value
            .as_str()
            .map(str::trim)
            .filter(|unit| !unit.is_empty())
            .ok_or_else(|| format!("`{path}.{field}` must be a non-empty string"))?;
        Ok(Some(unit.to_ascii_uppercase()))
    };
    let camel_unit = read_unit(window.get("timeUnit"), "timeUnit")?;
    let snake_unit = read_unit(window.get("time_unit"), "time_unit")?;
    if duration_value.is_none() && camel_unit.is_none() && snake_unit.is_none() {
        return Ok(None);
    }
    if let (Some(camel), Some(snake)) = (&camel_unit, &snake_unit) {
        if camel != snake {
            return Err(format!("`{path}.timeUnit` and `{path}.time_unit` disagree"));
        }
    }
    let duration = positive_number(duration_value, &format!("{path}.duration"))?;
    let unit = camel_unit
        .or(snake_unit)
        .ok_or_else(|| format!("missing `{path}.timeUnit`"))?;
    let minutes = if unit.contains("MINUTE") {
        duration
    } else if unit.contains("HOUR") {
        duration * MINUTES_PER_HOUR
    } else if unit.contains("DAY") {
        duration * MINUTES_PER_DAY
    } else if unit.contains("WEEK") {
        duration * DAYS_PER_WEEK * MINUTES_PER_DAY
    } else if unit.contains("SECOND") {
        duration / SECONDS_PER_MINUTE
    } else {
        return Err(format!("`{path}.timeUnit` has unsupported value `{unit}`"));
    };
    if !minutes.is_finite() || minutes >= i64::MAX as f64 {
        return Err(format!("`{path}.duration` is outside the supported range"));
    }
    Ok(Some(window_label_from_minutes(minutes)))
}
