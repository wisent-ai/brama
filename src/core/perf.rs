//! Process-local per-model performance telemetry.
//!
//! Successful chat dispatches record latency and output-token stats keyed by
//! the requested route id. `/v1/models` exposes them as an optional `perf`
//! block per route and `/stats` reports how many models are tracked.
//!
//! Stats are best-effort persisted to a JSON file (atomic rewrite after each
//! record) and reloaded on startup so a process restart keeps the numbers.
//! The file lives on the service host's instance-local `/tmp`, so numbers are
//! per process, never fleet-wide.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PerfStats {
    pub count: u64,
    pub total_latency_ms: f64,
    pub total_output_tokens: u64,
    pub last_latency_ms: f64,
    pub last_tps: f64,
}

/// Averaged per-model view handed to the HTTP layer.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelPerf {
    pub model: String,
    pub count: u64,
    /// Mean dispatch latency in milliseconds (totals / count).
    pub latency_ms: f64,
    /// Mean output tokens per second (total tokens / total latency).
    pub tps: f64,
    pub last_latency_ms: f64,
    /// Tokens/sec of the most recent dispatch.
    pub last_tps: f64,
}

static REGISTRY: LazyLock<Mutex<HashMap<String, PerfStats>>> = LazyLock::new(|| Mutex::new(load()));

fn persist_path() -> String {
    std::env::var("BRAMA_PERF_PATH").unwrap_or_else(|_| "/tmp/brama-perf.json".into())
}

fn load() -> HashMap<String, PerfStats> {
    std::fs::read_to_string(persist_path())
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

/// Record one successful dispatch for `model` (the requested route id).
pub fn record(model: &str, latency_ms: f64, output_tokens: u32) {
    let model = model.trim();
    if model.is_empty() || !latency_ms.is_finite() || latency_ms < 0.0 {
        return;
    }
    let tps = if latency_ms > 0.0 {
        f64::from(output_tokens) / (latency_ms / 1000.0)
    } else {
        0.0
    };
    let Ok(mut map) = REGISTRY.lock() else {
        return;
    };
    let stats = map.entry(model.to_string()).or_default();
    stats.count += 1;
    stats.total_latency_ms += latency_ms;
    stats.total_output_tokens += u64::from(output_tokens);
    stats.last_latency_ms = latency_ms;
    stats.last_tps = tps;
    flush(&map);
}

/// Atomic write: serialize to a sibling temp file, then rename over the target.
fn flush(map: &HashMap<String, PerfStats>) {
    let path = persist_path();
    let tmp = format!("{path}.tmp");
    let Ok(payload) = serde_json::to_vec(map) else {
        return;
    };
    if std::fs::write(&tmp, payload).is_ok() {
        let _ = std::fs::rename(&tmp, &path);
    }
}

fn average(model: &str, stats: &PerfStats) -> ModelPerf {
    let latency_ms = if stats.count > 0 {
        stats.total_latency_ms / stats.count as f64
    } else {
        0.0
    };
    let tps = if stats.total_latency_ms > 0.0 {
        stats.total_output_tokens as f64 / (stats.total_latency_ms / 1000.0)
    } else {
        0.0
    };
    ModelPerf {
        model: model.to_string(),
        count: stats.count,
        latency_ms,
        tps,
        last_latency_ms: stats.last_latency_ms,
        last_tps: stats.last_tps,
    }
}

/// All tracked models, most-used first.
pub fn snapshot() -> Vec<ModelPerf> {
    let Ok(map) = REGISTRY.lock() else {
        return Vec::new();
    };
    let mut out: Vec<ModelPerf> = map
        .iter()
        .map(|(model, stats)| average(model, stats))
        .collect();
    out.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.model.cmp(&b.model)));
    out
}

/// Averages for a single route id, when stats exist.
pub fn get(model: &str) -> Option<ModelPerf> {
    REGISTRY
        .lock()
        .ok()?
        .get(model)
        .map(|stats| average(model, stats))
}

/// Number of models currently tracked.
pub fn tracked_count() -> usize {
    REGISTRY.lock().map(|map| map.len()).unwrap_or(0)
}
