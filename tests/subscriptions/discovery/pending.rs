//! Replay facts retained by an actual failed gateway inventory pass.
//! Run on the gateway host with its real state directory; never manufacture pending state.

use super::{required, Run};
use serde_json::{json, Value};
use std::path::PathBuf;

#[test]
#[ignore = "Requires actual retained discovery observations and restored real vault inventory on the gateway host"]
fn retained_account_facts_reconcile_after_inventory_recovery() {
    let mut run = Run::new();
    let path = PathBuf::from(required("PENDING_STATE_DIR")).join("account-discovery/pending.json");
    let before: Value =
        serde_json::from_slice(&std::fs::read(&path).expect("read actual pending facts"))
            .expect("pending fact JSON");
    let provider = required("EXPECTED_PROVIDER");
    let account = required("EXPECTED_ACCOUNT");
    let retained = before
        .as_array()
        .expect("pending accounts")
        .iter()
        .find(|observation| {
            observation["provider"] == provider
                && observation["account"]
                    .as_str()
                    .is_some_and(|value| value.eq_ignore_ascii_case(&account))
        })
        .expect("selected account was actually retained before inventory recovery");
    run.report["pending_before"] = before.clone();
    run.save();
    let response = run.command(&["subscription", "discover"]);
    assert!(
        response.status.success(),
        "reconciliation refused: {} {}",
        String::from_utf8_lossy(&response.stdout),
        String::from_utf8_lossy(&response.stderr)
    );
    let result: Value = serde_json::from_slice(&response.stdout).expect("real discovery report");
    let reconciled = result["accounts"]
        .as_array()
        .expect("reconciled account rows")
        .iter()
        .find(|row| {
            row["provider"] == provider
                && row["account"]
                    .as_str()
                    .is_some_and(|value| value.eq_ignore_ascii_case(&account))
        })
        .expect("retained account was enrolled");
    let member = run.member(reconciled["id"].as_str().expect("exact member id"), false);
    assert_eq!(member["discovery"]["account"], retained["account"]);
    assert!(member["discovery"]["sources"]
        .as_array()
        .expect("retained sources")
        .contains(&retained["source"]));
    let discovered = member["discovery"]["discovered_at_ms"]
        .as_i64()
        .expect("discovery date");
    let observed = retained["observed_at_ms"]
        .as_i64()
        .expect("retained observation date");
    assert!(
        discovered <= observed,
        "replay lost the original discovery date"
    );
    let after: Value =
        serde_json::from_slice(&std::fs::read(&path).expect("read reconciled pending state"))
            .expect("pending state JSON");
    run.report["pending_after"] = after.clone();
    run.save();
    assert_eq!(
        after,
        json!([]),
        "successful reconciliation left pending observations"
    );
    run.report["result"] = json!("passed");
    run.save();
}
