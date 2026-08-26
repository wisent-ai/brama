#[path = "../support/mod.rs"]
mod support;

use std::io::Write;
use std::process::{Command, Stdio};

use serde_json::Value;
use support::TestDirectory;

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

#[test]
fn subscription_commands_read_isolated_state_and_require_explicit_mutation_context() {
    let directory = TestDirectory::new("cli-subscriptions");
    let list = command(&directory)
        .args(["subscriptions", "list", "--json"])
        .output()
        .expect("subscription list");
    assert!(
        list.status.success(),
        "{}",
        String::from_utf8_lossy(&list.stderr)
    );
    let body: Value = serde_json::from_slice(&list.stdout).expect("subscription list JSON");
    assert_eq!(body["providers"], Value::Array(Vec::new()));

    let refresh = command(&directory)
        .args([
            "subscription",
            "refresh",
            "openai",
            "--reason",
            "contract verifies an empty provider pool",
            "--json",
        ])
        .output()
        .expect("subscription refresh");
    assert!(!refresh.status.success());
    let body: Value = serde_json::from_slice(&refresh.stdout).expect("refresh verdict JSON");
    assert_eq!(body["provider"], "openai");
    assert_eq!(body["attempted"], 0);
    assert_eq!(body["result"], "failed");
    assert!(directory.path().join("state/journal.jsonl").is_file());
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
