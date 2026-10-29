//! Process-local per-model performance telemetry.
//!
//! Successful chat dispatches record latency and output-token stats keyed by
//! the requested route id. `/v1/models` exposes them as an optional `perf`
//! block per route and `/stats` reports how many models are tracked.
//!
//! Stats are persisted to a JSON file (atomic rewrite after each record) and
//! reloaded on startup so a process restart keeps the numbers. A file that
//! cannot be read, parsed, placed or written is logged (`perf_stats_*`
//! events) with its path and cause; serving never waits on it. The file lives
//! in Brama's state directory on the service host, so numbers are per host,
//! never fleet-wide. A rate that has no time to be measured over (a dispatch
//! answered in no measurable time, a model with no dispatch) is absent, never
//! zero.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex, MutexGuard};

use serde::{Deserialize, Serialize};

/// Milliseconds in a second, the SI prefix milli:
/// https://www.bipm.org/en/publications/si-brochure
const MILLISECONDS_PER_SECOND: f64 = 1000.0;

/// One recorded dispatch adds one to its model's count.
// https://en.wikipedia.org/wiki/Counting : each counted item adds one.
const ONE_DISPATCH: u64 = 1;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PerfStats {
    pub count: u64,
    pub total_latency_ms: f64,
    pub total_output_tokens: u64,
    pub last_latency_ms: f64,
    /// Tokens per second of the most recent dispatch, absent when it was
    /// answered in no measurable time.
    pub last_tps: Option<f64>,
}

/// Averaged per-model view handed to the HTTP layer.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelPerf {
    pub model: String,
    pub count: u64,
    /// Mean dispatch latency in milliseconds (totals / count), absent for a
    /// model with no dispatch counted.
    pub latency_ms: Option<f64>,
    /// Mean output tokens per second (total tokens / total latency), absent
    /// while no latency was measured.
    pub tps: Option<f64>,
    pub last_latency_ms: f64,
    /// Tokens/sec of the most recent dispatch.
    pub last_tps: Option<f64>,
}

static REGISTRY: LazyLock<Mutex<HashMap<String, PerfStats>>> = LazyLock::new(|| Mutex::new(load()));

/// `$BRAMA_PERF_PATH`, else `perf.json` in Brama's state directory: never the
/// shared temporary directory, whose files any account may replace. Neither
/// declared nor a state directory to hold it is the state directory's cause.
fn persist_path() -> Result<String, String> {
    let path = match std::env::var_os("BRAMA_PERF_PATH").filter(|path| !path.is_empty()) {
        Some(path) => std::path::PathBuf::from(path),
        None => crate::journal::state_dir()?.join("perf.json"),
    };
    Ok(path.display().to_string())
}

/// Where the stats persist, resolved once: the environment and the home do
/// not change while the process runs, so a path that cannot be placed is
/// logged once with its cause and the stats then stay in memory only,
/// instead of the same warning on every dispatch.
static PERSIST_PATH: LazyLock<Option<String>> = LazyLock::new(|| match persist_path() {
    Ok(path) => Some(path),
    Err(error) => {
        tracing::warn!(event = "perf_stats_unplaced", %error, "the stats stay in memory only and are not kept");
        None
    }
});

/// The stats are counters a panic mid-record leaves at worst one dispatch
/// short, so a poisoned lock is recovered rather than served as no stats.
fn registry() -> MutexGuard<'static, HashMap<String, PerfStats>> {
    crate::core::sync::held(&REGISTRY, "perf stats")
}

/// An absent file is a first start; one that cannot be placed, read or
/// parsed is logged with the path and cause, so the next flush replacing it
/// is seen instead of the numbers quietly starting over.
fn load() -> HashMap<String, PerfStats> {
    let Some(path) = PERSIST_PATH.as_deref() else {
        return HashMap::new();
    };
    let raw = match std::fs::read_to_string(path) {
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

/// Output tokens per second over `latency_ms`, absent when no time passed to
/// measure a rate over.
fn rate(tokens: f64, latency_ms: f64) -> Option<f64> {
    latency_ms
        .is_normal()
        .then(|| tokens / (latency_ms / MILLISECONDS_PER_SECOND))
}

/// Record one successful dispatch for `model` (the requested route id). A
/// dispatch without a route id or with a latency that is not a finite,
/// non-negative number is not counted, and said so.
pub fn record(model: &str, latency_ms: f64, output_tokens: u32) {
    let model = model.trim();
    if model.is_empty() || !latency_ms.is_finite() || latency_ms.is_sign_negative() {
        tracing::warn!(
            event = "perf_stats_uncounted",
            model,
            latency_ms,
            "a dispatch without a route id or a measurable latency is not counted"
        );
        return;
    }
    let mut map = registry();
    let stats = map.entry(model.to_string()).or_default();
    let Some(count) = stats.count.checked_add(ONE_DISPATCH) else {
        tracing::warn!(
            event = "perf_stats_uncounted",
            model,
            "the dispatch count cannot grow further"
        );
        return;
    };
    stats.count = count;
    stats.total_latency_ms += latency_ms;
    stats.total_output_tokens = stats
        .total_output_tokens
        .saturating_add(u64::from(output_tokens));
    stats.last_latency_ms = latency_ms;
    stats.last_tps = rate(f64::from(output_tokens), latency_ms);
    flush(&map);
}

/// Atomic write: serialize to a sibling temp file, then rename over the target.
/// A step that fails is logged with the path and cause; the stats in memory
/// stay and the next record tries again.
fn flush(map: &HashMap<String, PerfStats>) {
    let Some(path) = PERSIST_PATH.as_deref() else {
        return;
    };
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
    if let Err(error) = std::fs::rename(&tmp, path) {
        tracing::warn!(event = "perf_stats_unwritten", %path, %error);
    }
}

fn average(model: &str, stats: &PerfStats) -> ModelPerf {
    ModelPerf {
        model: model.to_string(),
        count: stats.count,
        latency_ms: {
            let count = stats.count as f64;
            count.is_normal().then(|| stats.total_latency_ms / count)
        },
        tps: rate(stats.total_output_tokens as f64, stats.total_latency_ms),
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
