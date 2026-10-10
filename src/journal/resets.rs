//! A reset is irreversible capacity consumption: persist intent before asking the provider.

use crate::subscription_dispatch::acquire::resets::model::ResetRedemption;
use serde_json::{json, Value};
use std::fs::{File, OpenOptions};
use std::io::Write;

/// The operating system releases the member lock even if a caller crashes.
pub struct MemberLock {
    _file: File,
}

pub fn lock(member: &str) -> Result<MemberLock, String> {
    let directory = super::state_dir().join("reset-locks");
    std::fs::create_dir_all(&directory).map_err(|error| {
        format!(
            "create reset lock directory {}: {error}",
            directory.display()
        )
    })?;
    let path = directory.join(crate::gateway::broker::slug(member));
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .read(true)
        .open(&path)
        .map_err(|error| format!("open reset lock for {member}: {error}"))?;
    file.try_lock().map_err(|error| {
        format!("reset for {member} cannot acquire its exclusive lock: {error}")
    })?;
    Ok(MemberLock { _file: file })
}

pub fn record(outcome: &ResetRedemption) -> Result<(), String> {
    let mut record = serde_json::to_value(outcome)
        .map_err(|error| format!("encode subscription reset journal entry: {error}"))?;
    record["kind"] = json!("subscription_reset");
    record["at"] = json!(super::now());
    let path = &*super::PATH;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("create journal directory {}: {error}", parent.display()))?;
    }
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|error| format!("open reset journal {}: {error}", path.display()))?;
    let line = format!("{record}\n");
    file.write_all(line.as_bytes())
        .and_then(|()| file.sync_all())
        .map_err(|error| format!("persist reset journal {}: {error}", path.display()))
}

/// A damaged history is not an empty history: never spend again on that assumption.
pub fn latest(member: &str) -> Result<Option<ResetRedemption>, String> {
    let path = &*super::PATH;
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("read reset journal {}: {error}", path.display())),
    };
    let mut latest = None;
    for line in text.lines().filter(|line| !line.trim().is_empty()) {
        let entry: Value = serde_json::from_str(line)
            .map_err(|error| format!("decode reset journal {}: {error}", path.display()))?;
        if entry["kind"] == "subscription_reset" && entry["member"] == member {
            latest = Some(
                serde_json::from_value(entry)
                    .map_err(|error| format!("decode reset history for {member}: {error}"))?,
            );
        }
    }
    Ok(latest)
}
