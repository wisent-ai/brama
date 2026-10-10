//! Isolated real gateway with an obsolete member already holding a real lease.
use crate::support::{required, Run};
use serde_json::{json, Value};

#[test]
#[ignore = "Requires isolated gateway with real discovery and a leased obsolete member"]
fn discovery_preserves_members_with_active_session_leases() {
    let mut run = Run::new();
    let member = required("RECONCILE_LEASED_MEMBER");
    let before = run.snapshot_member(&member);
    let inventory = run.command(&["subscription", "lease", "list"]);
    assert!(inventory.status.success());
    let leases: Value = serde_json::from_slice(&inventory.stdout).expect("real lease inventory");
    let held: Vec<_> = leases["leases"]
        .as_array()
        .expect("live leases")
        .iter()
        .filter(|lease| lease["subscription_id"] == member)
        .cloned()
        .collect();
    assert!(!held.is_empty(), "fixture member must hold an actual lease");
    let output = run.command(&["subscription", "discover"]);
    assert!(
        !output.status.success(),
        "discovery removed a leased member"
    );
    let report: Value = serde_json::from_slice(&output.stdout).expect("refused discovery JSON");
    assert_eq!(report["ok"], false);
    assert!(report["errors"]
        .as_array()
        .expect("refusal causes")
        .iter()
        .any(|cause| cause.as_str().is_some_and(
            |cause| cause.contains(&member) && cause.contains("active session leases")
        )));
    let after = run.snapshot_member(&member);
    assert_eq!(after["id"], before["id"]);
    assert_eq!(after["account"], before["account"]);
    assert_eq!(after["provider"], before["provider"]);
    assert_eq!(after["credential"], before["credential"]);
    assert_eq!(after["retired"], before["retired"]);
    let inventory = run.command(&["subscription", "lease", "list"]);
    assert!(inventory.status.success());
    let after_leases: Value = serde_json::from_slice(&inventory.stdout).expect("persisted leases");
    let rows = after_leases["leases"]
        .as_array()
        .expect("live leases after refusal");
    for lease in held {
        assert!(rows.contains(&lease), "refusal changed a live lease");
    }
    run.report["refusal"] = report;
    run.report["member_before"] = before;
    run.report["member_after"] = after;
    run.report["result"] = json!("passed");
    run.save();
}
