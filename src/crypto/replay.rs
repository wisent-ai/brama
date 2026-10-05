//! Each signed agent request is accepted once, without a clock window.
//!
//! Per agent the guard keeps the newest signed timestamp it accepted and the
//! signatures accepted at that timestamp. A request is new when it is signed
//! later than that, or at that timestamp with a signature not seen yet; any
//! other request is a replay. Nothing here is a tolerance: an agent's own
//! clock orders its requests, and the guard only remembers what it accepted.
//! The record lives beside the journal in `$BRAMA_STATE_DIR` (default
//! `$HOME/.brama`), so a restart does not reopen old signatures.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::{LazyLock, Mutex};

use serde::{Deserialize, Serialize};

#[derive(Default, Deserialize, Serialize)]
struct Accepted {
    timestamp: i64,
    signatures: BTreeSet<String>,
}

fn record_path() -> PathBuf {
    let base = std::env::var("BRAMA_STATE_DIR")
        .ok()
        .map(|dir| dir.trim().to_string())
        .filter(|dir| !dir.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".to_string())).join(".brama")
        });
    base.join("agent-replay.json")
}

static GUARD: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

/// Accept `signature` for `agent_id` at `timestamp` once, or say why not.
pub fn accept_once(agent_id: &str, timestamp: i64, signature: &str) -> Result<(), String> {
    let _held = GUARD
        .lock()
        .map_err(|_| "the replay record lock was poisoned".to_string())?;
    let path = record_path();
    let mut record: BTreeMap<String, Accepted> = match fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|error| {
            format!(
                "the replay record {} is unreadable: {error}",
                path.display()
            )
        })?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => BTreeMap::new(),
        Err(error) => {
            return Err(format!(
                "the replay record {} could not be read: {error}",
                path.display()
            ))
        }
    };
    let accepted = record.entry(agent_id.to_string()).or_default();
    if timestamp < accepted.timestamp
        || (timestamp == accepted.timestamp && accepted.signatures.contains(signature))
    {
        return Err(format!(
            "replayed or out-of-order request: agent {agent_id} already had a request accepted signed at {}",
            accepted.timestamp
        ));
    }
    if timestamp > accepted.timestamp {
        accepted.timestamp = timestamp;
        accepted.signatures.clear();
    }
    accepted.signatures.insert(signature.to_string());
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("{} could not be created: {error}", parent.display()))?;
    }
    let staged = path.with_extension("json.tmp");
    let bytes = serde_json::to_vec(&record).map_err(|error| error.to_string())?;
    fs::write(&staged, bytes)
        .and_then(|()| fs::set_permissions(&staged, fs::Permissions::from_mode(0o600)))
        .and_then(|()| fs::rename(&staged, &path))
        .map_err(|error| {
            format!(
                "the replay record {} could not be written: {error}",
                path.display()
            )
        })
}
