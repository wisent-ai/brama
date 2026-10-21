//! Reading one field of a pool report the way every printed line reads it:
//! an instant as a time or as not stated, a string as present or absent.

use serde_json::Value;

/// An instant the report stated, or that it stated none, never a word that
/// reads as a time.
pub(in super::super) fn instant(value: Option<&Value>) -> String {
    let stated = value
        .and_then(Value::as_i64)
        .filter(|value| *value > 0)
        .and_then(chrono::DateTime::from_timestamp_millis)
        .map(|at| at.to_rfc3339());
    match stated {
        Some(at) => at,
        None => "not stated".to_string(),
    }
}

/// One string field, absent when the report states nothing there. A `null` reads
/// as absent, which is what the pool report writes for an expiry a credential
/// does not state and for a refusal there has not been.
pub(in super::super) fn text<'a>(report: &'a Value, key: &str) -> Option<&'a str> {
    report.get(key).and_then(Value::as_str)
}
