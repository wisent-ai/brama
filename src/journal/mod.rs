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

/// Brama's own state directory on this host: `$BRAMA_STATE_DIR`, or
/// `$HOME/.brama`. The journal, the agent replay record and the model
/// catalog copy all live here, never in a shared temporary directory.
pub(crate) fn state_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("BRAMA_STATE_DIR") {
        if !dir.trim().is_empty() {
            return PathBuf::from(dir.trim());
        }
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".brama")
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

fn field<'a>(record: &'a Value, key: &str) -> &'a str {
    record.get(key).and_then(Value::as_str).unwrap_or("")
}

/// Mark a subscription entry (by broker item id) retired so dispatch skips it.
pub fn retire(item_id: &str) -> Result<(), String> {
    append(json!({"kind": "retire", "id": item_id, "at": now()}))
}

/// Retain why obsolete membership was removed without retiring its account.
pub fn remove_membership(item_id: &str) -> Result<(), String> {
    append(json!({"kind": "membership_removed", "id": item_id, "at": now()}))
}

/// Only reconciliation tombstones can be replaced by later account discovery.
pub fn membership_was_removed(item_id: &str) -> Result<bool, String> {
    Ok(read_all()?
        .iter()
        .rev()
        .filter(|record| field(record, "id") == item_id)
        .find(|record| {
            matches!(
                field(record, "kind"),
                "retire" | "reinstate" | "membership_removed"
            )
        })
        .is_some_and(|record| field(record, "kind") == "membership_removed"))
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
        .filter(|record| field(record, "id") == item_id)
        .filter_map(|record| match field(record, "kind") {
            "retire" | "membership_removed" => Some(true),
            "reinstate" => Some(false),
            _ => None,
        })
        .next_back()
        == Some(true))
}

/// Record one operator-run credential refresh, with the reason they gave.
///
/// A refresh mutates state every later request depends on: the provider
/// invalidates the previous refresh token the moment it issues a new one, so a
/// grant rotated by hand is a grant nothing else can go back to. The reason is
/// stored beside the verdict because the question after a pool recovers -- or
/// stays empty -- is who asked for this and what the product told them, and
/// neither the provider's logs nor the ledger can answer that.
///
/// Every attempt is recorded, including one that found nothing to refresh. No
/// credential material is written here, exactly as everywhere else in this file.
pub fn record_subscription_refresh(
    provider: &str,
    reason: &str,
    result: &str,
    attempted: usize,
    detail: &str,
) -> Result<(), String> {
    append(json!({
        "kind": "subscription_refresh",
        "provider": provider,
        "reason": reason,
        "result": result,
        "attempted": attempted,
        "detail": detail,
        "at": now(),
    }))
}

/// Retain the exact non-secret authentication outcome, including its source
/// revision, failed stage and Weles run id. Timestamps alone cannot say what ran.
pub fn record_subscription_sign_in(outcome: &Value) -> Result<(), String> {
    let mut record = outcome.clone();
    record["kind"] = json!("subscription_sign_in");
    record["at"] = json!(now());
    record["at_ms"] = json!(chrono::Utc::now().timestamp_millis());
    append(record)
}

/// The newest completed automatic or operator sign-in for one subscription.
pub fn latest_subscription_sign_in(subscription_id: &str) -> Result<Option<Value>, String> {
    Ok(read_all()?.into_iter().rev().find(|record| {
        field(record, "kind") == "subscription_sign_in"
            && field(record, "subscription_id") == subscription_id
    }))
}

/// When the newest completed sign-in for one subscription happened, or `None`
/// when none ever has.
///
/// Distinct from "some time ago": a caller deciding whether another browser
/// sign-in could produce a different answer needs the instant, not an elapsed
/// window, because what it compares against is when the stored credential's
/// verdict was recorded.
pub fn latest_subscription_sign_in_at_ms(subscription_id: &str) -> Result<Option<i64>, String> {
    Ok(latest_subscription_sign_in(subscription_id)?.map(|latest| {
        latest
            .get("at_ms")
            .and_then(Value::as_i64)
            .unwrap_or_default()
    }))
}

/// Whether another browser sign-in may start after the persisted cooldown.
///
/// The journal, not process memory, is authoritative so restarting Brama cannot
/// turn one refusal into repeated Google sign-in and 2FA prompts.
pub fn subscription_sign_in_due(
    subscription_id: &str,
    cooldown: std::time::Duration,
) -> Result<bool, String> {
    let Some(at_ms) = latest_subscription_sign_in_at_ms(subscription_id)? else {
        return Ok(true);
    };
    let cooldown_ms = i64::try_from(cooldown.as_millis()).unwrap_or(i64::MAX);
    Ok(chrono::Utc::now().timestamp_millis().saturating_sub(at_ms) >= cooldown_ms)
}

/// Retain one account acquisition: the accounts counted, the plan read, the
/// Weles run and its stages, and how it ended. Written when the purchase is
/// requested (`requested`) and again when it ends, so a purchase whose answer
/// never came back is still on record as asked for.
pub fn record_subscription_acquisition(outcome: &Value) -> Result<(), String> {
    let mut record = outcome.clone();
    record["kind"] = json!("subscription_acquisition");
    record["at"] = json!(now());
    record["at_ms"] = json!(chrono::Utc::now().timestamp_millis());
    append(record)
}

/// The newest acquisition record for one provider, of any result.
pub fn latest_subscription_acquisition(provider: &str) -> Result<Option<Value>, String> {
    Ok(read_all()?.into_iter().rev().find(|record| {
        field(record, "kind") == "subscription_acquisition" && field(record, "provider") == provider
    }))
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
            field(r, "kind") == "check"
                && field(r, "agent_id") == agent_id
                && field(r, "task") == task
        })
        .collect())
}

/// All task-quality observations for one agent.
pub fn checks_for_agent(agent_id: &str) -> Result<Vec<Value>, String> {
    Ok(read_all()?
        .into_iter()
        .filter(|r| field(r, "kind") == "check" && field(r, "agent_id") == agent_id)
        .collect())
}
