//! The three subscription providers, tested against the real accounts.
//!
//! `claude-code`, `codex` and `kimi` are the agent-owned OAuth subscriptions,
//! and each one makes the same three claims an operator relies on: the
//! subscription serves a real completion, a refresh rotates the real grant at
//! the provider's token endpoint, and a sign-in through Weles re-authorizes a
//! disowned account. One story per claim per provider, no stubs, no dry runs.
//! Every assertion reads state -- the usage ledger on disk, the journal, the
//! exit code -- with stdout as supporting evidence only.
//!
//! These tests inherit the caller's environment on purpose. They need the
//! deployment's own vault, the launcher-installed capability environment, and
//! (for the sign-ins) the host Weles runs on -- exactly the real components
//! the flows use in production, which is why nothing here is isolated into a
//! tempdir. They mutate shared real accounts, so a lock inside this file
//! serializes them whatever thread count the runner uses:
//!
//! ```console
//! $ scripts/start-with-skarbiec.sh --exec cargo test --test subscription_real
//! ```
//!
//! The real-world costs, stated here so nobody discovers them from a bill:
//! each completion spends plan quota on its account, each refresh rotates the
//! account's refresh token -- every one of these providers invalidates the
//! previous token on issue, so this must run against the vault the serving
//! gateway reads, never against a stale copy of the account -- and each
//! sign-in drives a real browser session through Weles.

use std::path::PathBuf;
use std::process::Command;

use serde_json::Value;

const AGENT: &str = "wisent-app";

/// One real pool, one flow at a time: every test holds this for its whole
/// story, so the runner's thread count cannot interleave two mutations of the
/// same vault and ledger.
static REAL_ACCOUNT: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// The real binary with the real environment: no overrides, no tempdir. What
/// the launcher provided is what the flow gets.
fn brama() -> Command {
    Command::new(env!("CARGO_BIN_EXE_brama"))
}

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

/// Every row of the real ledger belonging to one provider, by subscription id.
fn provider_rows(provider: &str) -> Vec<(String, Value)> {
    let Ok(text) = std::fs::read_to_string(ledger_path()) else {
        return Vec::new();
    };
    let ledger: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    ledger["subscriptions"]
        .as_object()
        .map(|rows| {
            rows.iter()
                .filter(|(_, row)| row["provider"] == provider)
                .map(|(id, row)| (id.clone(), row.clone()))
                .collect()
        })
        .unwrap_or_default()
}

fn newest_journal_record(kind: &str) -> Option<Value> {
    let text = std::fs::read_to_string(journal_path()).ok()?;
    text.lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .rfind(|record| record["kind"] == kind)
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

fn now_ms() -> i64 {
    i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock")
            .as_millis(),
    )
    .expect("epoch millis fit i64")
}

/// One real completion served by one provider's subscription pool, proven by
/// the ledger: real traffic increments the measured counters of a row of that
/// provider, so nothing else can have paid for the response.
fn serves_a_real_completion(provider: &str, model_route: &str) {
    let _account = REAL_ACCOUNT
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let before = provider_rows(provider);
    let requests_before = total_requests(&before);

    let output = brama()
        .args([
            "test",
            "--model",
            model_route,
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
        "the {provider} subscription did not serve: {stdout}{stderr}"
    );
    assert!(stdout.contains("Model:"), "{stdout}");
    assert!(stdout.contains("Response:"), "{stdout}");
    assert!(stdout.contains("Tokens:"), "{stdout}");

    let after = provider_rows(provider);
    assert!(
        total_requests(&after) > requests_before,
        "no {provider} subscription recorded the request; the completion was not served by the \
         subscription pool"
    );
    assert!(
        max_field(&after, "/measured/last_used_ms") > max_field(&before, "/measured/last_used_ms"),
        "no {provider} subscription moved its last_used_ms"
    );
}

/// One real rotation of one provider's grant, proven by the ledger recording a
/// newer rotation instant and a future expiry, and by the journaled verdict
/// carrying the reason verbatim.
fn refresh_rotates_the_real_grant(provider: &str) {
    let _account = REAL_ACCOUNT
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let before = provider_rows(provider);
    let rotated_before = max_field(&before, "/credential/refreshed_at_ms");
    let reason = format!("{provider} real-account test: prove rotation end to end");

    let output = brama()
        .args(["subscription", "refresh", provider, "--reason", &reason])
        .output()
        .expect("brama subscription refresh");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "the {provider} refresh obtained nothing: {stdout}{stderr}"
    );
    assert!(stdout.contains("result: refreshed"), "{stdout}");

    let after = provider_rows(provider);
    assert!(
        max_field(&after, "/credential/refreshed_at_ms") > rotated_before,
        "no {provider} credential recorded a newer rotation instant; the grant was not rotated"
    );
    assert!(
        max_field(&after, "/credential/expires_at_ms") > now_ms(),
        "the rotated {provider} grant states no future expiry"
    );

    let record =
        newest_journal_record("subscription_refresh").expect("a refresh must journal its verdict");
    assert_eq!(record["provider"], provider);
    assert_eq!(record["result"], "refreshed");
    assert_eq!(record["reason"], Value::String(reason));
}

/// One real re-authorization of one provider's account through Weles, proven
/// by the verdict naming the exact sign-in row, the nested refresh answering
/// `refreshed`, no credential left waiting for re-authorization, and the
/// journaled sign-in record.
fn sign_in_reauthorizes_the_real_account(provider: &str) {
    let _account = REAL_ACCOUNT
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let reason = format!("{provider} real-account test: prove re-authorization end to end");
    let output = brama()
        .args([
            "subscription",
            "sign-in",
            provider,
            "--reason",
            &reason,
            "--json",
        ])
        .output()
        .expect("brama subscription sign-in");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "the {provider} sign-in did not end in a refreshed credential: {stdout}{stderr}"
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

    let rows = provider_rows(provider);
    assert!(!rows.is_empty(), "the ledger lost its {provider} rows");
    for (id, row) in &rows {
        assert_ne!(
            row.pointer("/credential/state").and_then(Value::as_str),
            Some("needs_reauthorization"),
            "{id} still needs re-authorization after a confirmed sign-in and refresh"
        );
    }

    let record = newest_journal_record("subscription_sign_in")
        .expect("a sign-in that reached Weles must journal its verdict");
    assert_eq!(record["provider"], provider);
    assert_eq!(record["result"], "signed_in");
    assert_eq!(record["reason"], Value::String(reason));
}

#[test]
fn the_codex_subscription_serves_a_real_completion() {
    serves_a_real_completion("codex", "codex/gpt-5.5");
}

#[test]
fn refresh_rotates_the_real_codex_grant() {
    refresh_rotates_the_real_grant("codex");
}

#[test]
fn sign_in_reauthorizes_the_real_codex_account() {
    sign_in_reauthorizes_the_real_account("codex");
}

#[test]
fn the_claude_code_subscription_serves_a_real_completion() {
    serves_a_real_completion("claude-code", "claude-code/claude-sonnet-4-6");
}

#[test]
fn refresh_rotates_the_real_claude_code_grant() {
    refresh_rotates_the_real_grant("claude-code");
}

#[test]
fn sign_in_reauthorizes_the_real_claude_code_account() {
    sign_in_reauthorizes_the_real_account("claude-code");
}

#[test]
fn the_kimi_subscription_serves_a_real_completion() {
    serves_a_real_completion("kimi", "kimi/kimi-for-coding");
}

#[test]
fn refresh_rotates_the_real_kimi_grant() {
    refresh_rotates_the_real_grant("kimi");
}

#[test]
fn sign_in_reauthorizes_the_real_kimi_account() {
    sign_in_reauthorizes_the_real_account("kimi");
}
