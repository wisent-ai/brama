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
//! Reading also brings the usage-report verdict from older copies forward
//! in memory, so a pure read never rewrites somebody's history. Readings
//! without an observation instant remain without one: the file's later
//! modification time is not evidence that a provider reported them then.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;

use super::{now_ms, CheckSource, Ledger, LedgerState};

const USAGE_FILE_ENV: &str = "BRAMA_SUBSCRIPTION_USAGE_FILE";

fn usage_path() -> Result<PathBuf, String> {
    if let Some(configured) = crate::journal::declared_path(USAGE_FILE_ENV) {
        return Ok(configured);
    }
    let home = crate::journal::home().map_err(|cause| {
        format!("cannot resolve the subscription usage ledger path: {USAGE_FILE_ENV} is not set and {cause}")
    })?;
    Ok(home
        .join(".config")
        .join("brama")
        .join("subscription-usage.json"))
}

/// Read the ledger once, retaining any failure beside the in-memory state.
///
/// A missing file is a new ledger. An existing file that cannot be read or
/// decoded is different: serving can continue from memory, but writing an empty
/// replacement would destroy the only copy of its history.
pub(super) fn load() -> LedgerState {
    let path = match usage_path() {
        Ok(path) => path,
        Err(cause) => {
            return LedgerState {
                ledger: Ledger::default(),
                storage_error: Some(cause),
                persist_blocked: true,
            }
        }
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

/// Write through a private temporary file in the same directory and rename, so
/// a reader never sees a half-written ledger and a crash never truncates one.
pub(super) fn persist(ledger: &Ledger) -> Result<(), String> {
    let path = usage_path()?;
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
            options.mode(u32::from(libc::S_IRUSR | libc::S_IWUSR));
        }
        let mut file = options.open(&staging)?;
        file.write_all(&bytes)?;
        file.write_all(b"\n")?;
        file.sync_all()
    })();
    if let Err(error) = written {
        return Err(crate::core::staging::failed_with(
            format!(
                "cannot write subscription usage ledger staging file `{}`: {error}",
                staging.display()
            ),
            &staging,
        ));
    }
    if let Err(error) = fs::rename(&staging, &path) {
        return Err(crate::core::staging::failed_with(
            format!(
                "cannot replace subscription usage ledger `{}` from `{}`: {error}",
                path.display(),
                staging.display()
            ),
            &staging,
        ));
    }
    Ok(())
}
