//! Where the ledger's one durable copy lives, what reading it yields, and how
//! a whole new copy replaces it.
//!
//! The file is the reason this module survives a restart, so the two rules it
//! is written under are the ones an operator's history depends on. A copy that
//! exists but cannot be read or decoded is reported rather than overwritten,
//! because the only record of how much of this month is gone is the one on
//! disk. And a copy is only ever replaced whole, through a private staging file
//! in the same directory, so a reader never sees half a ledger and a crash
//! never truncates one.
//!
//! Reading also brings older copies forward: readings written before they
//! carried their own instant, and the usage-report verdict that lived in the
//! completion probe's field before it had one of its own. Both are done in
//! memory on load, so a pure read never rewrites somebody's history.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::time::UNIX_EPOCH;

use super::{now_ms, CheckSource, Ledger, LedgerState};

const USAGE_FILE_ENV: &str = "BRAMA_SUBSCRIPTION_USAGE_FILE";

fn usage_path() -> Option<PathBuf> {
    if let Ok(configured) = std::env::var(USAGE_FILE_ENV) {
        let trimmed = configured.trim();
        if !trimmed.is_empty() {
            return Some(PathBuf::from(trimmed));
        }
    }
    let home = std::env::var("HOME").ok()?;
    Some(
        PathBuf::from(home)
            .join(".config")
            .join("brama")
            .join("subscription-usage.json"),
    )
}

/// Read the ledger once, retaining any failure beside the in-memory state.
///
/// A missing file is a new ledger. An existing file that cannot be read or
/// decoded is different: serving can continue from memory, but writing an empty
/// replacement would destroy the only copy of its history.
pub(super) fn load() -> LedgerState {
    let Some(path) = usage_path() else {
        return LedgerState {
            ledger: Ledger::default(),
            storage_error: Some(
                "cannot resolve the subscription usage ledger path because HOME is unavailable"
                    .to_string(),
            ),
            persist_blocked: true,
        };
    };
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return LedgerState {
                ledger: Ledger::default(),
                storage_error: None,
                persist_blocked: false,
            };
        }
        Err(error) => {
            return LedgerState {
                ledger: Ledger::default(),
                storage_error: Some(format!(
                    "cannot read subscription usage ledger `{}`: {error}",
                    path.display()
                )),
                persist_blocked: true,
            };
        }
    };
    let mut ledger: Ledger = match serde_json::from_str(&text) {
        Ok(ledger) => ledger,
        Err(error) => {
            return LedgerState {
                ledger: Ledger::default(),
                storage_error: Some(format!(
                    "cannot decode subscription usage ledger `{}`: {error}",
                    path.display()
                )),
                persist_blocked: true,
            };
        }
    };
    backfill_reading_times(&mut ledger, &path);
    // Before `usage_check` existed the usage-report verdict occupied `probe`.
    // Copy it into the dedicated field in memory so a later completion probe
    // cannot erase it. Pure reads do not rewrite the legacy file.
    for usage in ledger.subscriptions.values_mut() {
        if usage.usage_check.is_none()
            && usage
                .probe
                .as_ref()
                .is_some_and(|probe| probe.source == Some(CheckSource::UsageReport))
        {
            usage.usage_check = usage.probe.clone();
        }
    }
    LedgerState {
        ledger,
        storage_error: None,
        persist_blocked: false,
    }
}

/// Give readings written before `recorded_at_ms` existed the ledger file's own
/// timestamp.
///
/// Those readings were observed at some point before the file was last written,
/// so the file's modification time is the tightest upper bound available and is
/// closer to the truth than the epoch. It is an upper bound, not a measurement,
/// which is why it is only ever applied to a reading that carries no instant of
/// its own.
fn backfill_reading_times(ledger: &mut Ledger, path: &std::path::Path) {
    let file_written_at_ms = fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
        .and_then(|elapsed| i64::try_from(elapsed.as_millis()).ok())
        .unwrap_or_default();
    if file_written_at_ms == 0 {
        return;
    }
    for usage in ledger.subscriptions.values_mut() {
        for reading in usage.limits.values_mut() {
            if reading.recorded_at_ms == 0 {
                reading.recorded_at_ms = file_written_at_ms;
            }
        }
    }
}

/// Write through a private temporary file in the same directory and rename, so
/// a reader never sees a half-written ledger and a crash never truncates one.
pub(super) fn persist(ledger: &Ledger) -> Result<(), String> {
    let path = usage_path().ok_or_else(|| {
        "cannot resolve the subscription usage ledger path because HOME is unavailable".to_string()
    })?;
    let parent = path.parent().ok_or_else(|| {
        format!(
            "subscription usage ledger `{}` has no parent directory",
            path.display()
        )
    })?;
    fs::create_dir_all(parent).map_err(|error| {
        format!(
            "cannot create subscription usage ledger directory `{}`: {error}",
            parent.display()
        )
    })?;
    let bytes = serde_json::to_vec_pretty(ledger)
        .map_err(|error| format!("cannot encode subscription usage ledger: {error}"))?;
    let staging = parent.join(format!(
        ".subscription-usage.{}.{}.tmp",
        std::process::id(),
        now_ms()
    ));
    let written = (|| -> std::io::Result<()> {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&staging)?;
        file.write_all(&bytes)?;
        file.write_all(b"\n")?;
        file.sync_all()
    })();
    if let Err(error) = written {
        let _ = fs::remove_file(&staging);
        return Err(format!(
            "cannot write subscription usage ledger staging file `{}`: {error}",
            staging.display()
        ));
    }
    if let Err(error) = fs::rename(&staging, &path) {
        let _ = fs::remove_file(&staging);
        return Err(format!(
            "cannot replace subscription usage ledger `{}` from `{}`: {error}",
            path.display(),
            staging.display()
        ));
    }
    Ok(())
}
