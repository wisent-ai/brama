//! Requires a real synchronized purchase receipt for an account also in OMP.
//! RECEIPT_ID and MEMBER select existing records; no mail or grant is fabricated.
use super::{required, Run};
use serde_json::{json, Value};
use std::process::{Command, Stdio};

fn observe(run: &mut Run, program: &str, arguments: &[&str]) -> Value {
    let mut command = Command::new(program);
    command.args(arguments).stdin(Stdio::null());
    let output = stado_wait::output(&mut command).expect("read actual account evidence");
    run.report["commands"].as_array_mut().unwrap().push(json!({
        "command": program, "arguments": arguments, "exit_status": output.status.code(),
        "stdout": String::from_utf8_lossy(&output.stdout),
        "stderr": String::from_utf8_lossy(&output.stderr),
    }));
    run.save();
    assert!(
        output.status.success(),
        "{program} refused: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("actual product JSON")
}

#[test]
#[ignore = "Requires a real gateway, readable purchase receipt, working receipt inference and OMP account"]
fn historical_purchase_does_not_replace_current_reported_plan() {
    let mut run = Run::new();
    let member = required("MEMBER");
    let receipt_id = required("RECEIPT_ID");
    let account = required("EXPECTED_ACCOUNT");
    let harness_provider = required("EXPECTED_HARNESS_PROVIDER");
    let receipt = observe(&mut run, "skrzynka", &["message", "show", &receipt_id]);
    let sent_at = chrono::DateTime::parse_from_rfc3339(
        receipt["sent_at"].as_str().expect("real receipt date"),
    )
    .expect("provider receipt timestamp")
    .timestamp_millis();
    let mailbox = receipt["mailbox_id"].as_str().expect("receipt mailbox");
    let source = format!("skrzynka:{mailbox}:{receipt_id}");
    let usage = observe(&mut run, "omp", &["usage", "--json"]);
    let row = usage["reports"]
        .as_array()
        .expect("actual harness reports")
        .iter()
        .find(|row| {
            row["provider"] == harness_provider
                && row["metadata"]["email"]
                    .as_str()
                    .is_some_and(|email| email.eq_ignore_ascii_case(&account))
        })
        .expect("the receipt account is currently reported by the harness");
    let plan = row["metadata"]["planType"]
        .as_str()
        .expect("current provider plan");
    let observed_at = chrono::Utc::now().timestamp_millis();
    assert!(
        sent_at < observed_at,
        "the selected receipt must predate current usage"
    );
    let discovery = run.command(&["subscription", "discover"]);
    assert!(
        discovery.status.success(),
        "discovery refused: {} {}",
        String::from_utf8_lossy(&discovery.stdout),
        String::from_utf8_lossy(&discovery.stderr)
    );
    let answer: Value = serde_json::from_slice(&discovery.stdout).expect("discovery JSON");
    assert_eq!(answer["ok"], true, "{answer}");
    let persisted = run.member(&member, false);
    assert_eq!(persisted["discovery"]["account"], account.to_lowercase());
    assert!(
        persisted["discovery"]["sources"]
            .as_array()
            .expect("source evidence")
            .iter()
            .any(|value| value == &source),
        "selected receipt was not interpreted as this account's purchase"
    );
    assert_eq!(persisted["discovery"]["plan"], plan);
    assert!(
        persisted["discovery"]["plan_at_ms"]
            .as_i64()
            .expect("dated plan fact")
            > sent_at,
        "historical receipt replaced current usage as the plan source"
    );
    let repeated = run.command(&["subscription", "discover"]);
    assert!(
        repeated.status.success(),
        "repeat discovery refused: {} {}",
        String::from_utf8_lossy(&repeated.stdout),
        String::from_utf8_lossy(&repeated.stderr)
    );
    let after = run.member(&member, false);
    assert_eq!(after["discovery"]["plan"], plan);
    assert!(
        after["discovery"]["plan_at_ms"]
            .as_i64()
            .expect("retained plan date")
            > sent_at
    );
    run.report["result"] = json!("passed");
    run.save();
}
