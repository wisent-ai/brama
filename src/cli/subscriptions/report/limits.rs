//! The usage windows of one subscription in `brama subscription list`: how
//! much of each window is used, when it resets and when it was observed.
//! A row whose report carries no window list is said as such, apart from a
//! row whose list is empty, so an unreported measurement never reads as one
//! that found nothing.

use serde_json::Value;

use super::{instant, text};

/// A used share is printed per hundred
/// (https://en.wikipedia.org/wiki/Percentage).
const PER_HUNDRED: f64 = 100.0;

/// Every usage window of `row`, one line each.
pub(super) fn print_limits(row: &Value) {
    let Some(limits) = row.get("limits").and_then(Value::as_array) else {
        println!("    usage windows not reported");
        return;
    };
    if limits.is_empty() {
        println!("    no usage measurement available");
    }
    let stale = if row.get("stale").and_then(Value::as_bool) == Some(true) {
        " (stale)"
    } else {
        ""
    };
    for limit in limits {
        let amount = match limit.get("used_fraction").and_then(Value::as_f64) {
            Some(fraction) => format!("{:.1}% used", fraction * PER_HUNDRED),
            None => "used share not stated".to_string(),
        };
        let window = match text(limit, "window_label").or_else(|| text(limit, "label")) {
            Some(window) => window,
            None => "window without a name in the report",
        };
        println!(
            "    {window}: {amount}; reset {}; observed {}{stale}",
            instant(limit.get("resets_at_ms")),
            instant(limit.get("recorded_at_ms")),
        );
    }
}
