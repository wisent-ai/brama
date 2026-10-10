//! Exercise installed Claude Code metadata and the actual serving pool.
use super::{required, Run};
use serde_json::{json, Value};
use std::process::{Command, Stdio};

#[test]
#[ignore = "Requires Claude Code signed into the selected account and real discovery dependencies"]
fn native_claude_identity_persists_in_the_serving_pool() {
    let mut run = Run::new();
    let expected = required("NATIVE_CLAUDE_ACCOUNT");
    let mut command = Command::new("claude");
    command.args(["auth", "status"]).stdin(Stdio::null());
    let observed = stado_wait::output(&mut command).expect("read native harness status");
    run.report["harness_observation"] = json!({
        "command": ["claude", "auth", "status"],
        "exit_status": observed.status.code(),
        "stdout": String::from_utf8_lossy(&observed.stdout),
        "stderr": String::from_utf8_lossy(&observed.stderr),
    });
    run.save();
    assert!(
        observed.status.success(),
        "{}",
        run.report["harness_observation"]
    );
    let status: Value = serde_json::from_slice(&observed.stdout).expect("native status JSON");
    assert_eq!(status["loggedIn"], true);
    assert_eq!(status["authMethod"], "claude.ai");
    assert!(status["email"]
        .as_str()
        .expect("native account address")
        .eq_ignore_ascii_case(&expected));
    let plan = status["subscriptionType"].as_str().expect("native plan");
    let discovered = run.command(&["subscription", "discover"]);
    assert!(
        discovered.status.success(),
        "discovery refused: {} {}",
        String::from_utf8_lossy(&discovered.stdout),
        String::from_utf8_lossy(&discovered.stderr)
    );
    let discovery: Value = serde_json::from_slice(&discovered.stdout).expect("discovery JSON");
    assert_eq!(discovery["ok"], true);
    let account = discovery["accounts"]
        .as_array()
        .expect("enrolled accounts")
        .iter()
        .find(|row| {
            row["provider"] == "claude-code"
                && row["account"]
                    .as_str()
                    .is_some_and(|email| email.eq_ignore_ascii_case(&expected))
        })
        .expect("native account enrolled");
    assert_eq!(account["registered"], true);
    let member = run.member(account["id"].as_str().expect("member identity"), false);
    assert_eq!(member["discovery"]["account"], expected.to_lowercase());
    assert_eq!(member["discovery"]["plan"], plan);
    assert!(member["discovery"]["sources"]
        .as_array()
        .expect("persisted sources")
        .iter()
        .any(|source| source == "claude auth status"));
    run.report["result"] = json!("passed");
    run.save();
}
