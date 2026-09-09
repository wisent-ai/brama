//! What an ordinary answer said about the plan in its own headers, and how
//! those readings ride out with the response, refusal included.

use std::collections::HashMap;

use super::window::window_label_from_minutes;
use crate::types::{LimitReading, ModelResponse};

/// The response headers that carry plan state, lowercased, and nothing else.
///
/// Only the two families any provider on this fleet publishes are kept, so a
/// header sweep can never turn into an accidental log of provider metadata.
pub(in crate::providers::adapter) fn plan_headers(
    headers: &reqwest::header::HeaderMap,
) -> HashMap<String, String> {
    headers
        .iter()
        .filter_map(|(name, value)| {
            let name = name.as_str().to_ascii_lowercase();
            if !(name.starts_with("anthropic-ratelimit-unified-") || name.starts_with("x-codex-")) {
                return None;
            }
            Some((name, value.to_str().ok()?.to_string()))
        })
        .collect()
}

fn header_number(headers: &HashMap<String, String>, name: &str) -> Option<f64> {
    headers.get(name)?.trim().parse::<f64>().ok()
}

/// The wall-clock instant a provider answer was read, in milliseconds.
///
/// `Instant` is used for everything else in this family because everything else
/// here is a duration. A limit reading is stored and read back by another
/// process, so it needs an epoch timestamp its reader can compare against its
/// own clock.
pub(in crate::providers::adapter) fn observed_at_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or_default()
}

/// Turn one provider's plan headers into limit readings.
///
/// Anthropic publishes a utilization fraction and a reset instant per named
/// window; Codex publishes a used percentage, the window length in minutes and
/// a reset instant, per meter. Nothing is invented for providers that publish
/// neither -- an absent reading is absent, not zero.
pub(in crate::providers::adapter) fn limit_readings(
    provider_id: &str,
    headers: &HashMap<String, String>,
) -> Vec<LimitReading> {
    let seconds_to_ms = |seconds: f64| (seconds * 1_000.0).round() as i64;
    // One instant for every reading off one answer: they were all read from the
    // same set of headers, so a per-reading clock call would only invent
    // differences the provider never stated.
    let recorded_at_ms = observed_at_ms();
    match provider_id {
        "claude-code" | "anthropic" => ["5h", "7d"]
            .into_iter()
            .filter_map(|window| {
                let prefix = format!("anthropic-ratelimit-unified-{window}");
                let used = header_number(headers, &format!("{prefix}-utilization"))?;
                let resets = header_number(headers, &format!("{prefix}-reset"))
                    .filter(|value| *value > 0.0)
                    .map(seconds_to_ms);
                Some(LimitReading {
                    limit_id: format!("anthropic:{window}"),
                    label: match window {
                        "5h" => "Claude 5 hour".to_string(),
                        _ => "Claude 7 day".to_string(),
                    },
                    window_label: Some(match window {
                        "5h" => "5 hours".to_string(),
                        _ => "7 days".to_string(),
                    }),
                    used_fraction: used.clamp(0.0, 1.0),
                    resets_at_ms: resets,
                    recorded_at_ms,
                })
            })
            .collect(),
        "codex" | "openai-codex" => ["primary", "secondary"]
            .into_iter()
            .filter_map(|meter| {
                let percent = header_number(headers, &format!("x-codex-{meter}-used-percent"))?;
                let minutes = header_number(headers, &format!("x-codex-{meter}-window-minutes"));
                let resets = header_number(headers, &format!("x-codex-{meter}-reset-at"))
                    .filter(|value| *value > 0.0)
                    .map(seconds_to_ms);
                Some(LimitReading {
                    limit_id: format!("codex:{meter}"),
                    label: format!("Codex {meter} window"),
                    window_label: minutes.map(window_label_from_minutes),
                    used_fraction: (percent / 100.0).clamp(0.0, 1.0),
                    resets_at_ms: resets,
                    recorded_at_ms,
                })
            })
            .collect(),
        _ => Vec::new(),
    }
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
