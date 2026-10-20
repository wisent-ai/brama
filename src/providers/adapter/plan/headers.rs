//! What an ordinary answer said about the plan in its own headers, and how
//! those readings ride out with the response, refusal included.

use std::collections::HashMap;

use tracing::warn;

use super::window::window_label_from_minutes;
use crate::types::{LimitReading, ModelResponse};

/// Milliseconds in a second, for the reset instants headers state in seconds
/// and the latency a provider call is reported with.
pub(in crate::providers::adapter) const MS_PER_SECOND: f64 = 1_000.0; // https://www.bipm.org/en/measurement-units/si-prefixes milli = 10^-3
/// The share a percentage states per unit.
const PERCENT: f64 = 100.0; // https://www.iso.org/standard/64973.html ISO 80000-1: % = 0.01

/// The response headers that carry plan state, lowercased, and nothing else.
///
/// Only the two families any provider on this fleet publishes are kept, so a
/// header sweep can never turn into an accidental log of provider metadata.
/// A value that is not visible ASCII is kept as its bytes read, so the
/// reading below names it instead of the header vanishing.
pub(in crate::providers::adapter) fn plan_headers(
    headers: &reqwest::header::HeaderMap,
) -> HashMap<String, String> {
    headers
        .iter()
        .filter_map(|(name, value)| {
            let name = name.as_str().to_ascii_lowercase();
            (name.starts_with("anthropic-ratelimit-unified-") || name.starts_with("x-codex-"))
                .then(|| (name, String::from_utf8_lossy(value.as_bytes()).into_owned()))
        })
        .collect()
}

/// A plan header's number. A header the answer does not carry is absent; one
/// it carries that is not a number is absent too, and the gateway log names
/// the header, its value and the parser's reason.
fn header_number(headers: &HashMap<String, String>, name: &str) -> Option<f64> {
    let value = headers.get(name)?;
    match value.trim().parse::<f64>() {
        Ok(number) => Some(number),
        Err(error) => {
            warn!(
                event = "plan_header_unreadable",
                header = name,
                value = value.as_str(),
                "{name} is not a number: {error}"
            );
            None
        }
    }
}

/// A reset instant a header states in seconds since the epoch; a header that
/// states none, or no positive instant, names no reset.
fn reset_ms(headers: &HashMap<String, String>, name: &str) -> Option<i64> {
    header_number(headers, name)
        .filter(|seconds| seconds.is_sign_positive() && seconds.is_normal())
        .map(|seconds| (seconds * MS_PER_SECOND).round() as i64)
}

/// The wall-clock instant a provider answer was read, in milliseconds.
///
/// `Instant` is used for everything else in this family because everything else
/// here is a duration. A limit reading is stored and read back by another
/// process, so it needs an epoch timestamp its reader can compare against its
/// own clock.
pub(in crate::providers::adapter) fn observed_at_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// The names an answer gives its windows or meters: the part between
/// `prefix` and `suffix` of every header that has both.
fn named_between<'a>(
    headers: &'a HashMap<String, String>,
    prefix: &'a str,
    suffix: &'a str,
) -> impl Iterator<Item = &'a str> {
    headers
        .keys()
        .filter_map(move |name| name.strip_prefix(prefix)?.strip_suffix(suffix))
}

/// Turn one answer's plan headers into limit readings.
///
/// Anthropic publishes a utilization fraction and a reset instant per named
/// window; Codex publishes a used percentage, the window length in minutes and
/// a reset instant, per meter. Which family an answer carries, and which
/// windows or meters, is read from the header names themselves, so no list of
/// provider ids, windows or meters is kept beside what the provider sends: an
/// answer that carries neither family has no readings -- an absent reading is
/// absent, not zero. A used share is kept as the provider stated it, past the
/// whole limit included.
pub(in crate::providers::adapter) fn limit_readings(
    headers: &HashMap<String, String>,
) -> Vec<LimitReading> {
    // One instant for every reading off one answer: they were all read from the
    // same set of headers, so a per-reading clock call would only invent
    // differences the provider never stated.
    let recorded_at_ms = observed_at_ms();
    let anthropic = named_between(headers, "anthropic-ratelimit-unified-", "-utilization")
        .filter_map(|window| {
            let prefix = format!("anthropic-ratelimit-unified-{window}");
            let used = header_number(headers, &format!("{prefix}-utilization"))?;
            let limit_id = format!("anthropic:{window}");
            let (label, window_label) = match super::report::anthropic_window_labels(&limit_id) {
                Some((label, window_label)) => (label.to_string(), window_label.to_string()),
                None => (format!("Claude {window} window"), window.to_string()),
            };
            Some(LimitReading {
                limit_id,
                label,
                window_label: Some(window_label),
                used_fraction: used,
                resets_at_ms: reset_ms(headers, &format!("{prefix}-reset")),
                recorded_at_ms: Some(recorded_at_ms),
            })
        });
    let codex = named_between(headers, "x-codex-", "-used-percent").filter_map(|meter| {
        let percent = header_number(headers, &format!("x-codex-{meter}-used-percent"))?;
        let minutes = header_number(headers, &format!("x-codex-{meter}-window-minutes"));
        Some(LimitReading {
            limit_id: format!("codex:{meter}"),
            label: format!("Codex {meter} window"),
            window_label: minutes.map(window_label_from_minutes),
            used_fraction: percent / PERCENT,
            resets_at_ms: reset_ms(headers, &format!("x-codex-{meter}-reset-at")),
            recorded_at_ms: Some(recorded_at_ms),
        })
    });
    let mut readings: Vec<LimitReading> = anthropic.chain(codex).collect();
    readings.sort_by(|left, right| left.limit_id.cmp(&right.limit_id));
    readings
}

/// Carry the plan readings out with whatever response the call produced.
///
/// A rate-limited answer is the one that most needs them, so this is applied to
/// failures as well as successes rather than only on the happy path.
pub(in crate::providers::adapter) fn with_limits(
    mut response: ModelResponse,
    limits: Vec<LimitReading>,
) -> ModelResponse {
    response.limits = limits;
    response
}
