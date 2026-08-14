//! Process-local per-model performance telemetry.
//!
//! Successful chat dispatches record latency and output-token stats keyed by
//! the requested route id. `/v1/models` exposes them as an optional `perf`
//! block per route and `/stats` reports how many models are tracked.
//!
//! Stats are best-effort persisted to a JSON file (atomic rewrite, at most
//! every [`FLUSH_INTERVAL_MS`] ms) and reloaded on startup so a process
//! restart keeps recent numbers. The file lives on the service host's
//! instance-local `/tmp`, so numbers are per process, never fleet-wide.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{LazyLock, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// Hard cap on tracked models; the least-used entry is evicted beyond this.
const MAX_MODELS: usize = 500;
/// Minimum interval between persistence flushes.
const FLUSH_INTERVAL_MS: u64 = 30_000;
/// EMA smoothing factor applied to `last_tps` on each record.
const TPS_EMA_ALPHA: f64 = 0.3;

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
    /// EMA-smoothed tokens/sec of the most recent dispatches.
    pub last_tps: f64,
}

static REGISTRY: LazyLock<Mutex<HashMap<String, PerfStats>>> = LazyLock::new(|| Mutex::new(load()));
static LAST_FLUSH_MS: AtomicU64 = AtomicU64::new(0);

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn persist_path() -> String {
    std::env::var("BRAMA_PERF_PATH").unwrap_or_else(|_| "/tmp/brama-perf.json".into())
}

fn load() -> HashMap<String, PerfStats> {
    let mut map: HashMap<String, PerfStats> = std::fs::read_to_string(persist_path())
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default();
    if map.len() > MAX_MODELS {
        let mut entries: Vec<(String, PerfStats)> = map.into_iter().collect();
        entries.sort_by_key(|entry| std::cmp::Reverse(entry.1.count));
        entries.truncate(MAX_MODELS);
        map = entries.into_iter().collect();
    }
    map
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
    if !map.contains_key(model) && map.len() >= MAX_MODELS {
        if let Some(victim) = map
            .iter()
            .min_by_key(|(_, stats)| stats.count)
            .map(|(id, _)| id.clone())
        {
            map.remove(&victim);
        }
    }
    let stats = map.entry(model.to_string()).or_default();
    stats.count += 1;
    stats.total_latency_ms += latency_ms;
    stats.total_output_tokens += u64::from(output_tokens);
    stats.last_latency_ms = latency_ms;
    stats.last_tps = if stats.count == 1 {
        tps
    } else {
        TPS_EMA_ALPHA * tps + (1.0 - TPS_EMA_ALPHA) * stats.last_tps
    };
    maybe_flush(&map);
}

fn maybe_flush(map: &HashMap<String, PerfStats>) {
    let now = now_ms();
    let last = LAST_FLUSH_MS.load(Ordering::Relaxed);
    if now.saturating_sub(last) < FLUSH_INTERVAL_MS {
        return;
    }
    // Claim the flush slot so concurrent records do not duplicate the write.
    if LAST_FLUSH_MS
        .compare_exchange(last, now, Ordering::Relaxed, Ordering::Relaxed)
        .is_err()
    {
        return;
    }
    flush(map);
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

#[cfg(test)]
mod tests {
    use super::*;

    // Single test fn: the registry is process-global, so parallel test fns
    // would race on shared state.
    #[test]
    fn registry_end_to_end() {
        let persist = format!("/tmp/brama-perf-test-{}.json", std::process::id());
        std::env::set_var("BRAMA_PERF_PATH", &persist);
        let _ = std::fs::remove_file(&persist);

        record("test/model-a", 1000.0, 10);
        record("test/model-a", 3000.0, 30);
        record("test/model-b", 500.0, 5);

        let a = get("test/model-a").expect("model-a tracked");
        assert_eq!(a.count, 2);
        assert_eq!(a.latency_ms, 2000.0);
        assert_eq!(a.tps, 10.0); // 40 tokens over 4s
        assert_eq!(a.last_latency_ms, 3000.0);
        assert_eq!(a.last_tps, 10.0); // EMA of identical samples stays put

        // EMA smoothing: a faster third dispatch moves last_tps toward 40.
        record("test/model-a", 1000.0, 40);
        let a = get("test/model-a").expect("model-a tracked");
        let expected = 0.3 * 40.0 + 0.7 * 10.0;
        assert!(
            (a.last_tps - expected).abs() < 1e-9,
            "last_tps={}",
            a.last_tps
        );

        let snap = snapshot();
        let a_pos = snap.iter().position(|m| m.model == "test/model-a").unwrap();
        let b_pos = snap.iter().position(|m| m.model == "test/model-b").unwrap();
        assert!(a_pos < b_pos, "snapshot sorts by count desc");
        assert!(tracked_count() >= 2);

        // Persistence round-trip through the real flush/load helpers.
        {
            let map = REGISTRY.lock().expect("lock");
            flush(&map);
        }
        let loaded = load();
        assert_eq!(
            loaded.get("test/model-a").map(|s| s.count),
            Some(3),
            "flushed stats reload from disk"
        );
        let _ = std::fs::remove_file(&persist);

        // Cap: inserting beyond MAX_MODELS evicts a least-used entry.
        {
            let mut map = REGISTRY.lock().expect("lock");
            let fill = MAX_MODELS - map.len();
            for i in 0..fill {
                map.insert(
                    format!("test/filler-{i}"),
                    PerfStats {
                        count: 1,
                        ..PerfStats::default()
                    },
                );
            }
        }
        record("test/model-c", 100.0, 1);
        assert_eq!(tracked_count(), MAX_MODELS);
        assert!(get("test/model-c").is_some(), "new model survives eviction");

        // Garbage input is ignored.
        let before = tracked_count();
        record("", 100.0, 1);
        record("test/model-d", f64::NAN, 1);
        record("test/model-d", -5.0, 1);
        assert_eq!(tracked_count(), before);
    }
}
