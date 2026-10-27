//! Each signed agent request is accepted once, without a clock window.
//!
//! Per agent the guard keeps the newest signed timestamp it accepted and the
//! signatures accepted at that timestamp. A request is new when it is signed
//! later than that, or at that timestamp with a signature not seen yet; any
//! other request is a replay. Nothing here is a tolerance: an agent's own
//! clock orders its requests, and the guard only remembers what it accepted.
//! The record lives beside the journal in `$BRAMA_STATE_DIR` (default
//! `$HOME/.brama`), so a restart does not reopen old signatures.
//!
//! The record is read, decided and written whole under the exclusive lock
//! of [`crate::core::staging::held_beside`]. It was a mutex of this process,
//! so two Brama processes on one state directory (the service and a CLI
//! dispatch) could both read the record before either wrote it and both
//! accept one signature.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::core::staging::{held_beside, replace_private};

/// How the record is named in refusals.
const RECORD: &str = "the agent replay record";

#[derive(Default, Deserialize, Serialize)]
struct Accepted {
    timestamp: i64,
    signatures: BTreeSet<String>,
}

fn record_path() -> PathBuf {
    crate::journal::state_dir().join("agent-replay.json")
}

/// Accept `signature` for `agent_id` at `timestamp` once, or say why not.
pub fn accept_once(agent_id: &str, timestamp: i64, signature: &str) -> Result<(), String> {
    let path = record_path();
    let _held = held_beside(&path, RECORD)?;
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
    let bytes = serde_json::to_vec(&record)
        .map_err(|error| format!("the replay record {} cannot be encoded: {error}", path.display()))?;
    replace_private(&path, &bytes, RECORD)
}
