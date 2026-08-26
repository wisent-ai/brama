//! The OpenAI-account subscription (`codex`), tested against the real thing.
//!
//! Three stories, no stubs, no dry runs: a real completion served by the real
//! ChatGPT subscription, a real refresh that rotates the real grant at
//! auth.openai.com, and a real re-authorization driven by Weles in a real
//! browser. Every assertion reads state -- the usage ledger on disk, the
//! journal, the exit code -- with stdout as supporting evidence only.
//!
//! These tests inherit the caller's environment on purpose. They need the
//! deployment's own vault, the launcher-installed capability environment, and
//! (for the sign-in) the host Weles runs on -- exactly the real components the
//! flows use in production, which is why nothing here is isolated into a
//! tempdir. They mutate one shared real account, so a lock inside this file
//! serializes them whatever thread count the runner uses:
//!
//! ```console
//! $ scripts/start-with-skarbiec.sh env cargo test --test codex_subscription_real
//! ```
//!
//! Two of these tests have real-world costs, stated here so nobody discovers
//! them from a bill: the completion spends plan quota on the ChatGPT account,
//! and the refresh rotates the account's refresh token -- OpenAI invalidates
//! the previous one on issue, so this must run against the vault the serving
//! gateway reads, never against a stale copy of the account. The sign-in
//! drives a real browser session through Weles and counts against the
//! provider's session limits.

use std::path::PathBuf;
use std::process::Command;

use serde_json::Value;

const PROVIDER: &str = "codex";
/// A pinned row of the codex descriptor table; the test pins one model so a
/// failure names the route that failed instead of whatever a selector chose.
const MODEL_ROUTE: &str = "codex/gpt-5.5";
const AGENT: &str = "wisent-app";

/// The real binary with the real environment: no overrides, no tempdir. What
/// the launcher provided is what the flow gets.
fn brama() -> Command {
    Command::new(env!("CARGO_BIN_EXE_brama"))
}

/// One real account, one flow at a time: every test holds this for its whole
/// story, so the runner's thread count cannot interleave two mutations of the
/// same subscription.
static REAL_ACCOUNT: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// The usage ledger the deployment actually writes, resolved exactly the way
/// the product resolves it.
fn ledger_path() -> PathBuf {
    if let Ok(path) = std::env::var("BRAMA_SUBSCRIPTION_USAGE_FILE") {
        if !path.trim().is_empty() {
            return PathBuf::from(path.trim());
        }
    }
    let home = std::env::var("HOME").expect("HOME is required");
    PathBuf::from(home).join(".config/brama/subscription-usage.json")
}

fn journal_path() -> PathBuf {
    if let Ok(dir) = std::env::var("BRAMA_STATE_DIR") {
        if !dir.trim().is_empty() {
            return PathBuf::from(dir.trim()).join("journal.jsonl");
        }
    }
    let home = std::env::var("HOME").expect("HOME is required");
    PathBuf::from(home).join(".brama").join("journal.jsonl")
}

/// Every codex row of the real ledger, by subscription id.
fn codex_rows() -> Vec<(String, Value)> {
    let Ok(text) = std::fs::read_to_string(ledger_path()) else {
        return Vec::new();
    };
    let ledger: Value = serde_json::from_str(&text).unwrap_or_else(|_| Value::Null);
    ledger["subscriptions"]
        .as_object()
        .map(|rows| {
            rows.iter()
                .filter(|(_, row)| row["provider"] == PROVIDER)
                .map(|(id, row)| (id.clone(), row.clone()))
                .collect()
        })
        .unwrap_or_default()
}

fn newest_journal_record(kind: &str) -> Option<Value> {
    let text = std::fs::read_to_string(journal_path()).ok()?;
    text.lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|record| record["kind"] == kind)
        .next_back()
}

fn max_field(rows: &[(String, Value)], pointer: &str) -> i64 {
    rows.iter()
        .filter_map(|(_, row)| row.pointer(pointer).and_then(Value::as_i64))
        .max()
        .unwrap_or(0)
}

fn total_requests(rows: &[(String, Value)]) -> u64 {
    rows.iter()
        .filter_map(|(_, row)| row.pointer("/measured/requests").and_then(Value::as_u64))
        .sum()
}

#[test]
fn the_codex_subscription_serves_a_real_completion() {
    let _account = REAL_ACCOUNT.lock().expect("real-account lock");
    let before = codex_rows();
    assert!(
        !before.is_empty(),
        "the real ledger holds no codex subscription; this test proves the real account and \
         needs one signed in first"
    );
    let requests_before = total_requests(&before);

    let output = brama()
        .args([
            "test",
            "--model",
            MODEL_ROUTE,
            "--agent-id",
            AGENT,
            "--allow-provider-cost",
        ])
        .output()
        .expect("brama test");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "the subscription did not serve: {stdout}{stderr}"
    );
    // The printed contract of a served completion.
    assert!(stdout.contains("Model:"), "{stdout}");
    assert!(stdout.contains("Response:"), "{stdout}");
    assert!(stdout.contains("Tokens:"), "{stdout}");

    // The ledger is the statement that the subscription, not anything else,
    // paid for it: real traffic increments the measured counters.
    let after = codex_rows();
    assert!(
        total_requests(&after) > requests_before,
        "no codex subscription recorded the request; the completion was not served by the \
         subscription pool"
    );
    assert!(
        max_field(&after, "/measured/last_used_ms") > max_field(&before, "/measured/last_used_ms"),
        "no codex subscription moved its last_used_ms"
    );
}

#[test]
fn refresh_rotates_the_real_codex_grant() {
    let _account = REAL_ACCOUNT.lock().expect("real-account lock");
    let before = codex_rows();
    assert!(
        !before.is_empty(),
        "the real ledger holds no codex subscription; sign one in before proving rotation"
    );
    let rotated_before = max_field(&before, "/credential/refreshed_at_ms");

    let output = brama()
        .args([
            "subscription",
            "refresh",
            PROVIDER,
            "--reason",
            "codex real-account test: prove rotation end to end",
        ])
        .output()
        .expect("brama subscription refresh");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "the refresh obtained nothing: {stdout}{stderr}"
    );
    assert!(stdout.contains("result: refreshed"), "{stdout}");

    // The rotation is real only if the ledger records a newer rotation instant
    // and a future expiry for at least one codex grant.
    let after = codex_rows();
    let rotated_after = max_field(&after, "/credential/refreshed_at_ms");
    assert!(
        rotated_after > rotated_before,
        "no codex credential recorded a newer rotation instant; the grant was not rotated"
    );
    let now_ms = i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock")
            .as_millis(),
    )
    .expect("epoch millis fit i64");
    assert!(
        max_field(&after, "/credential/expires_at_ms") > now_ms,
        "the rotated grant states no future expiry"
    );

    // The audit trail: the exact reason beside the verdict, verbatim.
    let record = newest_journal_record("subscription_refresh")
        .expect("a refresh must journal its verdict");
    assert_eq!(record["provider"], PROVIDER);
    assert_eq!(record["result"], "refreshed");
    assert_eq!(
        record["reason"],
        "codex real-account test: prove rotation end to end"
    );
}

#[test]
fn sign_in_reauthorizes_the_real_codex_account() {
    let _account = REAL_ACCOUNT.lock().expect("real-account lock");
    let output = brama()
        .args([
            "subscription",
            "sign-in",
            PROVIDER,
            "--reason",
            "codex real-account test: prove re-authorization end to end",
            "--json",
        ])
        .output()
        .expect("brama subscription sign-in");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "the sign-in did not end in a refreshed credential: {stdout}{stderr}"
    );
    let verdict: Value = serde_json::from_str(stdout.trim()).expect("verdict is JSON");
    assert_eq!(verdict["result"], "signed_in", "{stdout}");
    assert_eq!(verdict["refresh"]["result"], "refreshed", "{stdout}");
    assert!(
        verdict["login_item"]
            .as_str()
            .is_some_and(|item| !item.is_empty()),
        "the verdict must name the exact sign-in row Weles drove: {stdout}"
    );

    // After a confirmed sign-in and its proving refresh, no codex credential
    // may still be waiting for re-authorization.
    let rows = codex_rows();
    assert!(!rows.is_empty(), "the ledger lost its codex rows");
    for (id, row) in &rows {
        assert_ne!(
            row.pointer("/credential/state").and_then(Value::as_str),
            Some("needs_reauthorization"),
            "{id} still needs re-authorization after a confirmed sign-in and refresh"
        );
    }

    // The audit trail carries the sign-in verdict beside the refresh it ran.
    let record = newest_journal_record("subscription_sign_in")
        .expect("a sign-in that reached Weles must journal its verdict");
    assert_eq!(record["provider"], PROVIDER);
    assert_eq!(record["result"], "signed_in");
    assert_eq!(
        record["reason"],
        "codex real-account test: prove re-authorization end to end"
    );
}
