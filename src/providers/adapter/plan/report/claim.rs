//! What a provider is allowed to claim in a report, and what every number,
//! percentage and instant must be before it is believed.

use serde_json::{Map, Value};

use super::{EPOCH_MILLISECONDS_FLOOR, MS_PER_SECOND, PERCENT_SCALE};

pub(super) fn required_object<'a>(
    value: Option<&'a Value>,
    path: &str,
) -> Result<&'a Map<String, Value>, String> {
    value
        .ok_or_else(|| format!("missing `{path}`"))?
        .as_object()
        .ok_or_else(|| format!("`{path}` must be an object"))
}

/// A claimed number may be JSON numeric or a decimal string, but it must be
/// finite. Accepting `NaN` from a string would poison every later comparison.
pub(super) fn claimed_number(value: Option<&Value>, path: &str) -> Result<f64, String> {
    let value = value.ok_or_else(|| format!("missing `{path}`"))?;
    let number = match value {
        Value::Number(number) => number
            .as_f64()
            .ok_or_else(|| format!("`{path}` is outside the supported numeric range"))?,
        Value::String(text) => text
            .trim()
            .parse::<f64>()
            .map_err(|error| format!("`{path}` is not a number: {error}"))?,
        _ => return Err(format!("`{path}` must be a number")),
    };
    if !number.is_finite() {
        return Err(format!("`{path}` must be finite"));
    }
    Ok(number)
}

pub(super) fn optional_number(value: Option<&Value>, path: &str) -> Result<Option<f64>, String> {
    match value {
        None | Some(Value::Null) => Ok(None),
        value => claimed_number(value, path).map(Some),
    }
}

pub(super) fn percentage(value: Option<&Value>, path: &str) -> Result<f64, String> {
    let percent = claimed_number(value, path)?;
    if !(0.0..=PERCENT_SCALE).contains(&percent) {
        return Err(format!("`{path}` must be between 0 and 100"));
    }
    Ok(percent / PERCENT_SCALE)
}

pub(super) fn positive_number(value: Option<&Value>, path: &str) -> Result<f64, String> {
    let number = claimed_number(value, path)?;
    if number <= 0.0 {
        return Err(format!("`{path}` must be greater than zero"));
    }
    Ok(number)
}

pub(super) fn nonnegative_number(value: Option<&Value>, path: &str) -> Result<f64, String> {
    let number = claimed_number(value, path)?;
    if number < 0.0 {
        return Err(format!("`{path}` must not be negative"));
    }
    Ok(number)
}

fn epoch_number_ms(number: f64, path: &str) -> Result<i64, String> {
    if !number.is_finite() || number <= 0.0 {
        return Err(format!("`{path}` must be a positive finite instant"));
    }
    let milliseconds = if number >= EPOCH_MILLISECONDS_FLOOR {
        number
    } else {
        number * MS_PER_SECOND
    };
    if !milliseconds.is_finite() || milliseconds >= i64::MAX as f64 {
        return Err(format!("`{path}` is outside the supported instant range"));
    }
    Ok(milliseconds.round() as i64)
}

/// An optional instant may be RFC 3339, epoch seconds, or epoch milliseconds.
/// A present but malformed claim is an error rather than an absent reset.
pub(super) fn optional_instant_ms(
    value: Option<&Value>,
    path: &str,
) -> Result<Option<i64>, String> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    if let Value::String(text) = value {
        let text = text.trim();
        if let Ok(instant) = chrono::DateTime::parse_from_rfc3339(text) {
            let milliseconds = instant.timestamp_millis();
            if milliseconds <= 0 {
                return Err(format!("`{path}` must be a positive instant"));
            }
            return Ok(Some(milliseconds));
        }
        let number = text
            .parse::<f64>()
            .map_err(|error| format!("`{path}` is not RFC 3339 or an epoch number: {error}"))?;
        return epoch_number_ms(number, path).map(Some);
    }
    epoch_number_ms(claimed_number(Some(value), path)?, path).map(Some)
}

pub(super) fn delayed_reset_ms(
    seconds: f64,
    recorded_at_ms: i64,
    path: &str,
) -> Result<i64, String> {
    if !seconds.is_finite() || seconds < 0.0 {
        return Err(format!("`{path}` must be a nonnegative finite duration"));
    }
    let milliseconds = seconds * MS_PER_SECOND;
    if !milliseconds.is_finite() || milliseconds >= i64::MAX as f64 {
        return Err(format!("`{path}` is outside the supported duration range"));
    }
    recorded_at_ms
        .checked_add(milliseconds.round() as i64)
        .ok_or_else(|| format!("`{path}` overflows the reset instant"))
}
