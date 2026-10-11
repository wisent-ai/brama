//! The plan windows a harness usage row relays about one account.

use serde_json::Value;

use crate::types::LimitReading;

/// The plan windows one harness usage row relays, each read at the instant
/// the harness fetched the report (`fetchedAt`). A row that states no fetch
/// instant, or a limit without its used fraction, is refused by name: a
/// fraction that cannot be aged, or a window with no number, must not be
/// served as a reading.
pub(super) fn windows(
    report: &Value,
    source: &str,
    harness_provider: &str,
    account: &str,
) -> Result<Vec<LimitReading>, String> {
    let Some(limits) = report.get("limits").and_then(Value::as_array) else {
        return Ok(Vec::new());
    };
    if limits.is_empty() {
        return Ok(Vec::new());
    }
    let fetched_at_ms = report
        .get("fetchedAt")
        .and_then(Value::as_i64)
        .ok_or_else(|| {
            format!(
                "{source}: {harness_provider} {account} states plan windows but no fetchedAt, so \
                 they cannot be aged and were not recorded"
            )
        })?;
    let mut readings = Vec::with_capacity(limits.len());
    for limit in limits {
        let limit_id = limit
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("{source}: {harness_provider} {account}: a limit has no id"))?;
        let used_fraction = limit
            .pointer("/amount/usedFraction")
            .and_then(Value::as_f64)
            .ok_or_else(|| {
                format!(
                    "{source}: {harness_provider} {account}: limit {limit_id} states no \
                     amount.usedFraction, so no reading was recorded"
                )
            })?;
        readings.push(LimitReading {
            limit_id: limit_id.to_owned(),
            label: limit
                .get("label")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    format!("{source}: {harness_provider} {account}: limit {limit_id} has no label")
                })?
                .to_owned(),
            window_label: limit
                .pointer("/window/label")
                .and_then(Value::as_str)
                .map(str::to_owned),
            used_fraction,
            resets_at_ms: limit.pointer("/window/resetsAt").and_then(Value::as_i64),
            recorded_at_ms: fetched_at_ms,
        });
    }
    Ok(readings)
}
