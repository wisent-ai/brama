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

pub(in crate::core::server) static TOTAL_REQUESTS: AtomicU64 = AtomicU64::new(0);
pub(in crate::core::server) static TOTAL_INPUT_TOKENS: AtomicU64 = AtomicU64::new(0);
pub(in crate::core::server) static TOTAL_OUTPUT_TOKENS: AtomicU64 = AtomicU64::new(0);
pub(in crate::core::server) static TOTAL_PROVIDER_ATTEMPTS: AtomicU64 = AtomicU64::new(u64::MIN);
pub(in crate::core::server) static TOTAL_FAILURES: AtomicU64 = AtomicU64::new(u64::MIN);
pub(in crate::core::server) static STARTED_AT: LazyLock<Instant> = LazyLock::new(Instant::now);

pub(in crate::core::server) fn record_typed_request(attempts: u32, failed: bool) {
    TOTAL_REQUESTS.fetch_add(u64::from(true), Ordering::Relaxed);
    TOTAL_PROVIDER_ATTEMPTS.fetch_add(attempts as u64, Ordering::Relaxed);
    if failed {
        TOTAL_FAILURES.fetch_add(u64::from(true), Ordering::Relaxed);
    }
}

fn typed_usage_tokens(body: &Value, keys: &[&str]) -> u64 {
    let Some(usage) = body.get("usage").and_then(Value::as_object) else {
        return u64::default();
    };
    keys.iter()
        .find_map(|key| usage.get(*key).and_then(Value::as_u64))
        .unwrap_or_default()
}

pub(in crate::core::server) fn record_typed_usage(body: &Value) {
    TOTAL_INPUT_TOKENS.fetch_add(
        typed_usage_tokens(body, &["prompt_tokens", "input_tokens"]),
        Ordering::Relaxed,
    );
    TOTAL_OUTPUT_TOKENS.fetch_add(
        typed_usage_tokens(body, &["completion_tokens", "output_tokens"]),
        Ordering::Relaxed,
    );
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
