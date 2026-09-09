//! The command surface of the built binary, driven as an operator drives it.
//!
//! Two inventories appear here on purpose, because the pool answers them
//! oppositely: a vault Brama cannot read at all, and a real Skarbiec vault
//! holding accounts. The unreadable one is an absent dependency -- a router
//! path that does not exist -- and not a stand-in for Skarbiec; telling those
//! two apart is the point of the first story.

#[path = "../support/mod.rs"]
mod support;

use std::io::Write;
use std::process::{Command, Stdio};

use serde_json::Value;
use support::{SkarbiecVault, TestDirectory};

/// The agent the real vault names as owner of every seeded account.
const SEEDED_AGENT: &str = "brama-cli-contracts";

/// The same command, over a real Skarbiec vault instead of an absent router.
fn command_over_vault(directory: &TestDirectory, vault: &SkarbiecVault) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_brama"));
    for (name, value) in vault.environment() {
        command.env(name, value);
    }
    command
        .env("XDG_STATE_HOME", directory.path().join("xdg-state"))
        .env("BRAMA_STATE_DIR", directory.path().join("state"))
        .env(
            "BRAMA_SUBSCRIPTION_USAGE_FILE",
            directory.path().join("usage.json"),
        )
        .env("BRAMA_PERF_PATH", directory.path().join("perf.json"))
        .env("ENTITLEMENTS_ROUTER_BIN", vault.router());
    command
}

/// The console's read of a pool that really holds accounts, and the other half
/// of `the_pool_reports_an_unreadable_inventory_instead_of_an_empty_one`: an
/// empty pool, an unreadable pool and a populated pool are three different
/// statements, and the product makes all three.
#[test]
fn the_pool_reports_one_row_per_account_the_vault_declares() {
    let directory = TestDirectory::new("cli-pool-seeded");
    let vault = SkarbiecVault::create("cli-pool-seeded");
    let providers = ["openai", "anthropic", "claude-code"];
    let rows: Vec<String> = providers
        .iter()
        .map(|provider| format!(r#""probe-{provider}":{{"provider":"{provider}"}}"#))
        .collect();
    std::fs::write(
        directory.path().join("usage.json"),
        format!(r#"{{"subscriptions":{{{}}}}}"#, rows.join(",")),
    )
    .expect("seed the usage ledger");
    for provider in providers {
        vault.seed_subscription(SEEDED_AGENT, provider, &format!("probe-{provider}"));
    }
    let before = std::fs::read(directory.path().join("usage.json")).expect("seeded ledger");

    let listed = command_over_vault(&directory, &vault)
        .args(["subscriptions", "--json"])
        .output()
        .expect("brama subscriptions --json");
    // An account nobody has read plan usage for makes the report incomplete
    // and says so per account, so the console read exits 1: a missing
    // measurement is never reported as zero usage.
    assert_eq!(listed.status.code(), Some(1));
    let report: Value = serde_json::from_slice(&listed.stdout).expect("pool report JSON");
    assert_eq!(report["scope"], "deployment");
    let answered = report["subscriptions"].as_array().expect("pool rows");
    assert_eq!(answered.len(), providers.len());
    for row in answered {
        let provider = row["provider"].as_str().expect("provider");
        assert!(providers.contains(&provider), "unexpected {provider}");
        assert_eq!(row["state"], "unknown");
        assert_eq!(row["id"], Value::String(format!("probe-{provider}")));
        assert_eq!(row["expires_at"], Value::Null);
        assert_eq!(row["last_redeem_error"], Value::Null);
    }
    // Copied from the live answer: one usage failure per account, saying the
    // measurement is missing rather than zero, plus one automatic-sign-in
    // failure for the `claude-code` account, whose seeded item names no Weles
    // account for a provider the gateway signs in by itself.
    let reported = report["errors"].as_array().expect("errors array");
    for provider in providers {
        assert!(
            reported
                .iter()
                .any(
                    |failure| failure["context"]["subscription"] == format!("probe-{provider}")
                        && failure["failure_point"] == "brama.subscriptions.usage"
                        && failure["detail"] == "usage has not been read for this subscription"
                ),
            "{report}"
        );
    }
    assert!(
        reported.iter().any(
            |failure| failure["context"]["blocked_by"] == "no_weles_account"
                && failure["context"]["subscription"] == "probe-claude-code"
        ),
        "{report}"
    );
    assert_eq!(reported.len(), providers.len() + 1, "{report}");

    let after = std::fs::read(directory.path().join("usage.json")).expect("ledger after the read");
    assert_eq!(before, after, "a listing must not rewrite the ledger");
    assert!(!directory.path().join("state/journal.jsonl").exists());
}

fn command(directory: &TestDirectory) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_brama"));
    command
        .env("HOME", directory.path().join("home"))
        .env("XDG_STATE_HOME", directory.path().join("xdg-state"))
        .env("BRAMA_STATE_DIR", directory.path().join("state"))
        .env(
            "BRAMA_SUBSCRIPTION_USAGE_FILE",
            directory.path().join("usage.json"),
        )
        .env(
            "BRAMA_DONATED_SUBSCRIPTIONS_FILE",
            directory.path().join("subscriptions.json"),
        )
        .env("BRAMA_PERF_PATH", directory.path().join("perf.json"))
        .env(
            "ENTITLEMENTS_ROUTER_BIN",
            directory.path().join("absent-router"),
        );
    command
}

#[test]
fn version_and_detect_report_the_exact_binary_and_host() {
    let directory = TestDirectory::new("cli-identity");
    let version = command(&directory)
        .arg("version")
        .output()
        .expect("brama version");
    assert!(
        version.status.success(),
        "{}",
        String::from_utf8_lossy(&version.stderr)
    );
    let body: Value = serde_json::from_slice(&version.stdout).expect("version JSON");
    assert_eq!(body["product"], "brama");
    assert_eq!(body["version"], env!("CARGO_PKG_VERSION"));
    assert!(body["source_revision"]
        .as_str()
        .is_some_and(|value| !value.is_empty()));

    let detect = command(&directory)
        .arg("detect")
        .output()
        .expect("brama detect");
    assert!(
        detect.status.success(),
        "{}",
        String::from_utf8_lossy(&detect.stderr)
    );
    let output = String::from_utf8_lossy(&detect.stdout);
    for field in [
        "GPU Type:",
        "RAM:",
        "CPU Cores:",
        "Recommended model:",
        "Recommended backend:",
    ] {
        assert!(output.contains(field), "missing {field} in {output}");
    }
}

#[test]
fn mcp_exposes_only_the_read_only_hardware_tool() {
    let directory = TestDirectory::new("cli-mcp");
    let mut child = command(&directory)
        .arg("mcp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start MCP server");
    child
        .stdin
        .take()
        .expect("MCP stdin")
        .write_all(
            b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\"}\n{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/list\"}\n{\"jsonrpc\":\"2.0\",\"id\":3,\"method\":\"tools/call\",\"params\":{\"name\":\"brama_detect\",\"arguments\":{}}}\n",
        )
        .expect("MCP requests");
    let output = child.wait_with_output().expect("MCP output");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let rows = String::from_utf8(output.stdout)
        .expect("MCP UTF-8")
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).expect("MCP JSON-RPC row"))
        .collect::<Vec<_>>();
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0]["result"]["serverInfo"]["name"], "brama");
    assert_eq!(
        rows[1]["result"]["tools"].as_array().expect("tools").len(),
        1
    );
    assert_eq!(rows[1]["result"]["tools"][0]["name"], "brama_detect");
    let detected = rows[2]["result"]["content"][0]["text"]
        .as_str()
        .expect("detection text");
    assert!(detected.contains("recommended_model"), "{detected}");
}

/// The console's own read of the pool, against state nothing has written yet.
///
/// The vault is unreachable here -- the router this directory names does not
/// exist -- and that is the point: an inventory Brama could not read is
/// reported as a failure with the reason it failed, and exits non-zero. It is
/// never flattened into an empty pool, because an empty pool and an unread
/// vault are the same picture and opposite repairs.
#[test]
fn the_pool_reports_an_unreadable_inventory_instead_of_an_empty_one() {
    let directory = TestDirectory::new("cli-subscriptions");
    let list = command(&directory)
        .args(["subscriptions", "--json"])
        .output()
        .expect("brama subscriptions");
    assert_eq!(list.status.code(), Some(1), "an incomplete pool exits 1");
    let body: Value = serde_json::from_slice(&list.stdout).expect("pool report JSON");
    assert_eq!(body["scope"], "deployment");
    assert_eq!(body["ok"], false);
    assert_eq!(body["subscriptions"], Value::Array(Vec::new()));
    assert_eq!(
        body["errors"][0]["failure_point"],
        "brama.subscriptions.discovery"
    );
    assert!(
        body["errors"][0]["detail"]
            .as_str()
            .is_some_and(|detail| detail.starts_with("list all subscriptions: ")),
        "the refusal must name the operation that failed: {body}"
    );
}

/// A repair that could not read the inventory it repairs refuses in the
/// inventory's own words and records nothing: journaling a verdict here would
/// record an attempt that never happened, and an operator reading the journal
/// during an incident would find a refresh that "failed" against a provider
/// nothing was ever asked about.
#[test]
fn subscription_refresh_refuses_with_the_inventory_reason_and_journals_nothing() {
    let directory = TestDirectory::new("cli-refresh-refusal");
    let refresh = command(&directory)
        .args([
            "subscription",
            "refresh",
            "openai",
            "--reason",
            "contract verifies a refusal before any provider is reached",
            "--json",
        ])
        .output()
        .expect("subscription refresh");
    assert_eq!(refresh.status.code(), Some(1));
    assert!(refresh.stdout.is_empty(), "a refusal prints no verdict");
    assert!(
        String::from_utf8_lossy(&refresh.stderr).starts_with("list all subscriptions: "),
        "{}",
        String::from_utf8_lossy(&refresh.stderr)
    );
    assert!(!directory.path().join("state/journal.jsonl").exists());
}

#[test]
fn billable_cli_commands_refuse_before_provider_access_without_cost_acknowledgement() {
    let directory = TestDirectory::new("cli-cost-boundary");
    let inference = command(&directory)
        .args([
            "test",
            "--model",
            "openai/default",
            "--agent-id",
            "contract-agent",
        ])
        .output()
        .expect("brama test refusal");
    assert!(!inference.status.success());
    assert!(String::from_utf8_lossy(&inference.stderr)
        .contains("refusing billable inference without explicit --allow-provider-cost"));

    let quality = command(&directory)
        .args([
            "collect-task-quality",
            "--agent-id",
            "contract-agent",
            "--task",
            "contract",
            "--prompt",
            "answer",
        ])
        .output()
        .expect("quality refusal");
    assert!(!quality.status.success());
    assert_eq!(
        String::from_utf8_lossy(&quality.stderr).trim(),
        "refusing billable task-quality collection without explicit cost acknowledgement"
    );
}
