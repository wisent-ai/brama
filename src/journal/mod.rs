//! Local append-only JSONL journal for non-secret Brama operational state:
//! retirement markers and task-quality observations used by the `task:`
//! selector. Credential material is never written here.
//! One file under `$BRAMA_STATE_DIR` (default `$HOME/.brama`); readers take the
//! last matching record.

use std::io::Write;
use std::path::PathBuf;
use std::sync::LazyLock;

use serde_json::{json, Value};

fn journal_path() -> PathBuf {
    if let Ok(dir) = std::env::var("BRAMA_STATE_DIR") {
        if !dir.trim().is_empty() {
            return PathBuf::from(dir.trim()).join("journal.jsonl");
        }
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".brama").join("journal.jsonl")
}

static PATH: LazyLock<PathBuf> = LazyLock::new(journal_path);

fn append(record: Value) {
    let path = &*PATH;
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let line = format!("{record}\n");
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = file.write_all(line.as_bytes());
    }
    std::process::Command::new("chmod")
        .arg("600")
        .arg(path)
        .status()
        .ok();
}

fn read_all() -> Vec<Value> {
    let Ok(text) = std::fs::read_to_string(&*PATH) else {
        return Vec::new();
    };
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .collect()
}

fn now() -> String {
    chrono::Utc::now().to_rfc3339()
}

fn field<'a>(record: &'a Value, key: &str) -> &'a str {
    record.get(key).and_then(Value::as_str).unwrap_or("")
}

/// Mark a subscription entry (by broker item id) retired so dispatch skips it.
pub fn retire(item_id: &str) {
    append(json!({"kind": "retire", "id": item_id, "at": now()}));
}

pub fn is_retired(item_id: &str) -> bool {
    read_all()
        .iter()
        .any(|r| field(r, "kind") == "retire" && field(r, "id") == item_id)
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
) {
    append(json!({
        "kind": "subscription_refresh",
        "provider": provider,
        "reason": reason,
        "result": result,
        "attempted": attempted,
        "detail": detail,
        "at": now(),
    }));
}

/// Record one subscription sign-in and the exact account Weles was asked to
/// drive. `subscription_id` is present for automatic renewal; an older
/// provider-wide CLI invocation may not have one.
pub fn record_subscription_sign_in(
    subscription_id: Option<&str>,
    provider: &str,
    login_item: &str,
    reason: &str,
    result: &str,
    detail: &str,
) {
    append(json!({
        "kind": "subscription_sign_in",
        "subscription_id": subscription_id,
        "provider": provider,
        "login_item": login_item,
        "reason": reason,
        "result": result,
        "detail": detail,
        "at": now(),
        "at_ms": chrono::Utc::now().timestamp_millis(),
    }));
}

/// The newest completed automatic or operator sign-in for one subscription.
pub fn latest_subscription_sign_in(subscription_id: &str) -> Option<Value> {
    read_all().into_iter().rev().find(|record| {
        field(record, "kind") == "subscription_sign_in"
            && field(record, "subscription_id") == subscription_id
    })
}

/// When the newest completed sign-in for one subscription happened, or `None`
/// when none ever has.
///
/// Distinct from "some time ago": a caller deciding whether another browser
/// sign-in could produce a different answer needs the instant, not an elapsed
/// window, because what it compares against is when the stored credential's
/// verdict was recorded.
pub fn latest_subscription_sign_in_at_ms(subscription_id: &str) -> Option<i64> {
    latest_subscription_sign_in(subscription_id).map(|latest| {
        latest
            .get("at_ms")
            .and_then(Value::as_i64)
            .unwrap_or_default()
    })
}

/// Whether another browser sign-in may start after the persisted cooldown.
///
/// The journal, not process memory, is authoritative so restarting Brama cannot
/// turn one refusal into repeated Google sign-in and 2FA prompts.
pub fn subscription_sign_in_due(subscription_id: &str, cooldown: std::time::Duration) -> bool {
    let Some(at_ms) = latest_subscription_sign_in_at_ms(subscription_id) else {
        return true;
    };
    let cooldown_ms = i64::try_from(cooldown.as_millis()).unwrap_or(i64::MAX);
    chrono::Utc::now().timestamp_millis().saturating_sub(at_ms) >= cooldown_ms
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
) {
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
    append(record);
}

/// Task-quality observations for one agent + task name.
pub fn checks_for_task(agent_id: &str, task: &str) -> Vec<Value> {
    read_all()
        .into_iter()
        .filter(|r| {
            field(r, "kind") == "check"
                && field(r, "agent_id") == agent_id
                && field(r, "task") == task
        })
        .collect()
}

/// All task-quality observations for one agent.
pub fn checks_for_agent(agent_id: &str) -> Vec<Value> {
    read_all()
        .into_iter()
        .filter(|r| field(r, "kind") == "check" && field(r, "agent_id") == agent_id)
        .collect()
}
