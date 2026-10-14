//! Local append-only JSONL journal for non-secret Brama operational state:
//! retirement markers and task-quality observations used by the `task:`
//! selector. Credential material is never written here.
//! One file under `$BRAMA_STATE_DIR` (default `$HOME/.brama`); readers take the
//! last matching record.

use std::io::Write;
use std::path::PathBuf;
use std::sync::LazyLock;

use serde_json::{json, Value};

pub mod answers;
pub mod concurrency;
pub mod resets;
mod subscriptions;

pub use subscriptions::{
    latest_subscription_acquisition, latest_subscription_sign_in,
    latest_subscription_sign_in_at_ms, record_subscription_acquisition,
    record_subscription_refresh, record_subscription_sign_in, subscription_sign_in_due,
};

/// Brama's own state directory on this host: `$BRAMA_STATE_DIR`, or
/// `.brama` under the user's home. The journal, the agent replay record and
/// the model catalog copy all live here, never in a shared temporary
/// directory.
pub(crate) fn state_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("BRAMA_STATE_DIR") {
        if !dir.trim().is_empty() {
            return PathBuf::from(dir.trim());
        }
    }
    home_dir().join(".brama")
}

/// The user's home: HOME, else the home the user database records. With
/// neither, Brama stops with the cause; reading `.` as home put retirement
/// markers, leases and replay records in whatever directory Brama started
/// from, where the next start under another directory never saw them.
pub fn home_dir() -> PathBuf {
    std::env::home_dir().expect(
        "neither HOME nor the user database names a home directory, so Brama cannot find its state or the Stado binary",
    )
}

fn journal_path() -> PathBuf {
    state_dir().join("journal.jsonl")
}

static PATH: LazyLock<PathBuf> = LazyLock::new(journal_path);

/// Append one record. Every failure names the journal and the OS cause: a
/// retirement or acquisition that was not written must not look written.
fn append(record: Value) -> Result<(), String> {
    let path = &*PATH;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| {
            format!(
                "cannot create the journal directory {}: {error}",
                parent.display()
            )
        })?;
    }
    let line = format!("{record}\n");
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|error| format!("cannot open the journal {}: {error}", path.display()))?;
    file.write_all(line.as_bytes())
        .map_err(|error| format!("cannot append to the journal {}: {error}", path.display()))?;
    let status = std::process::Command::new("chmod")
        .arg("600")
        .arg(path)
        .status()
        .map_err(|error| {
            format!(
                "cannot run chmod on the journal {}: {error}",
                path.display()
            )
        })?;
    if !status.success() {
        return Err(format!(
            "chmod could not restrict the journal {} to its owner: {status}",
            path.display()
        ));
    }
    Ok(())
}

/// Every record. Only a journal that does not exist yet is empty; an
/// unreadable journal or a line that is not JSON is refused with the path and
/// the cause, because reading it as empty answers "not retired" for a member
/// its owner retired.
fn read_all() -> Result<Vec<Value>, String> {
    let path = &*PATH;
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(format!(
                "cannot read the journal {}: {error}",
                path.display()
            ))
        }
    };
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            serde_json::from_str::<Value>(line).map_err(|error| {
                format!(
                    "the journal {} holds a line that is not JSON ({line}): {error}",
                    path.display()
                )
            })
        })
        .collect()
}

fn now() -> String {
    chrono::Utc::now().to_rfc3339()
}

/// A record's text field, or nothing when the record does not carry it as
/// text: a missing field matches no query, not one that asks for "".
fn field<'a>(record: &'a Value, key: &str) -> Option<&'a str> {
    record.get(key).and_then(Value::as_str)
}

/// Mark a subscription entry (by broker item id) retired so dispatch skips it.
pub fn retire(item_id: &str) -> Result<(), String> {
    append(json!({"kind": "retire", "id": item_id, "at": now()}))
}

/// Retain why obsolete membership was removed without retiring its account.
pub fn remove_membership(item_id: &str) -> Result<(), String> {
    append(json!({"kind": "membership_removed", "id": item_id, "at": now()}))
}

/// Retain why a member that existed only in the usage history was dropped
/// from the pool, in the operator's words. Read like a reconciliation
/// tombstone: dispatch skips the id, and later discovery may enrol the
/// account again.
pub fn forget(item_id: &str, reason: &str) -> Result<(), String> {
    append(json!({"kind": "forgotten", "id": item_id, "reason": reason, "at": now()}))
}

/// Only reconciliation tombstones and forgotten members can be replaced by
/// later account discovery.
pub fn membership_was_removed(item_id: &str) -> Result<bool, String> {
    Ok(read_all()?
        .iter()
        .rev()
        .filter(|record| field(record, "id") == Some(item_id))
        .find(|record| {
            matches!(
                field(record, "kind"),
                Some("retire" | "reinstate" | "membership_removed" | "forgotten")
            )
        })
        .is_some_and(|record| {
            matches!(
                field(record, "kind"),
                Some("membership_removed" | "forgotten")
            )
        }))
}

/// Take one retirement back, because this deployment uses that account
/// after all.
///
/// Retirement used to be permanent: `is_retired` answered yes to any
/// `retire` record ever written, so a member given back could never be used
/// again by any command. A pool could then hold accounts it skipped on
/// every request, and the only route back was editing a journal by hand.
///
/// The journal stays append-only: this writes a record beside the
/// retirement, and the newest of the two decides. A reinstatement is not a
/// credential: the member is in the rotation again and still needs a grant
/// of this gateway's own, which is what the sign-in commands obtain.
pub fn reinstate(item_id: &str) -> Result<(), String> {
    append(json!({"kind": "reinstate", "id": item_id, "at": now()}))
}

/// Whether dispatch skips this member: the newest of its retirement and
/// reinstatement records, not merely whether a retirement was ever written.
pub fn is_retired(item_id: &str) -> Result<bool, String> {
    Ok(read_all()?
        .iter()
        .filter(|record| field(record, "id") == Some(item_id))
        .filter_map(|record| match field(record, "kind")? {
            "retire" | "membership_removed" | "forgotten" => Some(true),
            "reinstate" => Some(false),
            _ => None,
        })
        .next_back()
        == Some(true))
}

/// Append one task-quality observation.
#[allow(clippy::too_many_arguments)]
pub fn record_check(
    agent_id: &str,
    provider: &str,
    model: &str,
    task: &str,
    source: &str,
    status: &str,
    score: Option<f64>,
    checked_at: &str,
) -> Result<(), String> {
    let mut record = json!({
        "kind": "check",
        "agent_id": agent_id,
        "provider": provider,
        "model": model,
        "task": task,
        "source": source,
        "status": status,
        "checked_at": checked_at,
    });
    if let Some(value) = score {
        record["score"] = json!(value);
    }
    append(record)
}

/// Task-quality observations for one agent + task name.
pub fn checks_for_task(agent_id: &str, task: &str) -> Result<Vec<Value>, String> {
    Ok(read_all()?
        .into_iter()
        .filter(|r| {
            field(r, "kind") == Some("check")
                && field(r, "agent_id") == Some(agent_id)
                && field(r, "task") == Some(task)
        })
        .collect())
}

/// All task-quality observations for one agent.
pub fn checks_for_agent(agent_id: &str) -> Result<Vec<Value>, String> {
    Ok(read_all()?
        .into_iter()
        .filter(|r| field(r, "kind") == Some("check") && field(r, "agent_id") == Some(agent_id))
        .collect())
}
