//! Every credential call that stands, as its readers see it: `/readyz`,
//! `GET /stats` and the console's snapshot list the same records from here,
//! one per call, so a vault child that never answers is named with its
//! operation, its process and how long it has stood wherever one looks.

use std::time::{Instant, SystemTime};

use serde_json::{json, Value};

/// One standing call as read at one moment.
pub(in crate::core::server) struct CallReading {
    target: String,
    operation: String,
    pid: Option<u32>,
    since: Instant,
    began: SystemTime,
    refused: bool,
    read_at: Instant,
}

/// Every call standing now, the longest-standing first.
pub(in crate::core::server) fn readings() -> Vec<CallReading> {
    let read_at = Instant::now();
    let mut readings: Vec<CallReading> = super::standing()
        .values()
        .map(|call| CallReading {
            target: call.target.clone(),
            operation: call.operation.clone(),
            pid: call.pid,
            since: call.since,
            began: call.began,
            refused: call.end.is_none(),
            read_at,
        })
        .collect();
    readings.sort_by_key(|reading| reading.since);
    readings
}

impl CallReading {
    /// The call as one JSON record: its target, its operation, the pid of
    /// the process it runs in (null for a call over HTTP), `since` on the
    /// wall clock in RFC 3339, and `refused` once a reading has ended it with
    /// the refusal while its process is still being reaped. `elapsed` adds
    /// `elapsed_seconds`, how long it had stood when it was read.
    pub(in crate::core::server) fn document(&self, elapsed: bool) -> Value {
        let mut document = json!({
            "target": self.target,
            "operation": self.operation,
            "pid": self.pid,
            "since": chrono::DateTime::<chrono::Utc>::from(self.began).to_rfc3339(),
            "refused": self.refused,
        });
        if elapsed {
            document["elapsed_seconds"] =
                json!(self.read_at.duration_since(self.since).as_secs());
        }
        document
    }
}
