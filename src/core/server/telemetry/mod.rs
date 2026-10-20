//! What this process has done since it started: five counters, the per-model
//! latency history, and the two writers that fold a typed call into them.
//!
//! The numbers live here rather than beside any one handler because every wire
//! format and both endpoints report into the same totals, and [`stats`] is the
//! one place they are read out.

pub(in crate::core::server) mod stats;

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::LazyLock;
use std::time::Instant;

use serde_json::{json, Value};

use crate::types::Refusal;

/// A counter that starts with nothing counted when the process does.
type Counter = LazyLock<AtomicU64>;

pub(in crate::core::server) static TOTAL_REQUESTS: Counter = LazyLock::new(AtomicU64::default);
pub(in crate::core::server) static TOTAL_INPUT_TOKENS: Counter = LazyLock::new(AtomicU64::default);
pub(in crate::core::server) static TOTAL_OUTPUT_TOKENS: Counter = LazyLock::new(AtomicU64::default);
pub(in crate::core::server) static TOTAL_PROVIDER_ATTEMPTS: Counter =
    LazyLock::new(AtomicU64::default);
pub(in crate::core::server) static TOTAL_FAILURES: Counter = LazyLock::new(AtomicU64::default);
pub(in crate::core::server) static STARTED_AT: LazyLock<Instant> = LazyLock::new(Instant::now);

/// One request, failure or provider call, as the totals count it.
const ONE: u64 = std::num::NonZeroU64::MIN.get();

/// One typed call its provider answered: a request and the one provider call
/// it made.
pub(in crate::core::server) fn record_typed_answer() {
    TOTAL_REQUESTS.fetch_add(ONE, Ordering::Relaxed);
    TOTAL_PROVIDER_ATTEMPTS.fetch_add(ONE, Ordering::Relaxed);
}

/// One refused typed call: a request and a failure, and the one provider call
/// it made unless Brama refused it before asking any provider.
pub(in crate::core::server) fn record_typed_refusal(refused: &Refusal) {
    TOTAL_REQUESTS.fetch_add(ONE, Ordering::Relaxed);
    TOTAL_FAILURES.fetch_add(ONE, Ordering::Relaxed);
    if refused.class.asked_provider() {
        TOTAL_PROVIDER_ATTEMPTS.fetch_add(ONE, Ordering::Relaxed);
    }
}

/// One typed decision, folded into the same totals every other request lands
/// in. A decision is one provider call whichever engine answered it, so it
/// counts as one attempt and its tokens are the provider's own; a count the
/// provider did not state adds nothing, as for every other answer.
pub(in crate::core::server) fn record_decision_request(
    input_tokens: Option<u32>,
    output_tokens: Option<u32>,
    failed: bool,
) {
    TOTAL_REQUESTS.fetch_add(ONE, Ordering::Relaxed);
    TOTAL_PROVIDER_ATTEMPTS.fetch_add(ONE, Ordering::Relaxed);
    if let Some(input_tokens) = input_tokens {
        TOTAL_INPUT_TOKENS.fetch_add(u64::from(input_tokens), Ordering::Relaxed);
    }
    if let Some(output_tokens) = output_tokens {
        TOTAL_OUTPUT_TOKENS.fetch_add(u64::from(output_tokens), Ordering::Relaxed);
    }
    if failed {
        TOTAL_FAILURES.fetch_add(ONE, Ordering::Relaxed);
    }
}

/// The tokens a typed answer's `usage` object states: the OpenAI typed wire
/// names them `prompt_tokens`/`completion_tokens`, a Responses-shaped body
/// `input_tokens`/`output_tokens`. A count the provider did not state adds
/// nothing.
pub(in crate::core::server) fn record_typed_usage(body: &Value) {
    let stated = |pointer: &str| body.pointer(pointer).and_then(Value::as_u64);
    if let Some(input) = stated("/usage/prompt_tokens").or_else(|| stated("/usage/input_tokens")) {
        TOTAL_INPUT_TOKENS.fetch_add(input, Ordering::Relaxed);
    }
    if let Some(output) =
        stated("/usage/completion_tokens").or_else(|| stated("/usage/output_tokens"))
    {
        TOTAL_OUTPUT_TOKENS.fetch_add(output, Ordering::Relaxed);
    }
}

/// Optional per-model telemetry block, present only when the route has stats.
pub(in crate::core::server) fn perf_json(model: &str) -> Option<serde_json::Value> {
    crate::core::perf::get(model).map(|perf| {
        json!({
            "count": perf.count,
            "latencyMs": perf.latency_ms,
            "tps": perf.tps,
            "lastLatencyMs": perf.last_latency_ms,
            "lastTps": perf.last_tps,
        })
    })
}
