//! Process-local per-model performance telemetry.
//!
//! Successful chat dispatches record latency and output-token stats keyed by
//! the requested route id. `/v1/models` exposes them as an optional `perf`
//! block per route and `/stats` reports how many models are tracked.
//!
//! Stats are persisted to a JSON file (atomic rewrite after each record) and
//! reloaded on startup so a process restart keeps the numbers. A file that
//! cannot be read, parsed or written is logged (`perf_stats_*` events) with
//! its path and cause; serving never waits on it.
//! The file lives on the service host's instance-local `/tmp`, so numbers are
//! per process, never fleet-wide.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex, MutexGuard};

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

/// `$BRAMA_PERF_PATH`, else `perf.json` in Brama's state directory: never the
/// shared temporary directory, whose files any account may replace.
fn persist_path() -> String {
    match std::env::var_os("BRAMA_PERF_PATH").filter(|path| !path.is_empty()) {
        Some(path) => std::path::PathBuf::from(path),
        None => crate::journal::state_dir().join("perf.json"),
    }
    .display()
    .to_string()
}

/// The stats are counters a panic mid-record leaves at worst one dispatch
/// short, so a poisoned lock is recovered rather than served as no stats.
fn registry() -> MutexGuard<'static, HashMap<String, PerfStats>> {
    crate::core::sync::held(&REGISTRY, "perf stats")
}

/// An absent file is a first start; one that cannot be read or does not parse
/// is logged with the path and cause, so the next flush replacing it is seen
/// instead of the numbers quietly starting over.
fn load() -> HashMap<String, PerfStats> {
    let path = persist_path();
    let raw = match std::fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return HashMap::new(),
        Err(error) => {
            tracing::warn!(event = "perf_stats_unreadable", %path, %error);
            return HashMap::new();
        }
    };
    match serde_json::from_str(&raw) {
        Ok(stats) => stats,
        Err(error) => {
            tracing::warn!(event = "perf_stats_unparseable", %path, %error);
            HashMap::new()
        }
    }
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
    let mut map = registry();
    let stats = map.entry(model.to_string()).or_default();
    stats.count += 1;
    stats.total_latency_ms += latency_ms;
    stats.total_output_tokens += u64::from(output_tokens);
    stats.last_latency_ms = latency_ms;
    stats.last_tps = tps;
    flush(&map);
}

/// Atomic write: serialize to a sibling temp file, then rename over the target.
/// A step that fails is logged with the path and cause; the stats in memory
/// stay and the next record tries again.
fn flush(map: &HashMap<String, PerfStats>) {
    let path = persist_path();
    let tmp = format!("{path}.tmp");
    let payload = match serde_json::to_vec(map) {
        Ok(payload) => payload,
        Err(error) => {
            tracing::warn!(event = "perf_stats_unserializable", %error);
            return;
        }
    };
    if let Some(parent) = std::path::Path::new(&path).parent() {
        if let Err(error) = std::fs::create_dir_all(parent) {
            tracing::warn!(event = "perf_stats_unwritten", path = %parent.display(), %error);
            return;
        }
    }
    if let Err(error) = std::fs::write(&tmp, payload) {
        tracing::warn!(event = "perf_stats_unwritten", path = %tmp, %error);
        return;
    }
    if let Err(error) = std::fs::rename(&tmp, &path) {
        tracing::warn!(event = "perf_stats_unwritten", %path, %error);
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
    let map = registry();
    let mut out: Vec<ModelPerf> = map
        .iter()
        .map(|(model, stats)| average(model, stats))
        .collect();
    out.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.model.cmp(&b.model)));
    out
}

/// Averages for a single route id, when stats exist.
pub fn get(model: &str) -> Option<ModelPerf> {
    registry().get(model).map(|stats| average(model, stats))
}

/// Number of models currently tracked.
pub fn tracked_count() -> usize {
    registry().len()
}
