//! What the built binary says about itself and what it exposes: the version
//! and host report, the MCP tool surface, and the refusal a billable command
//! answers with before it reaches a provider.

#[path = "../support/mod.rs"]
mod support;

#[path = "../support/cli.rs"]
mod cli;

use std::io::Write;
use std::process::Stdio;

use cli::command;
use serde_json::Value;
use support::TestDirectory;

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
