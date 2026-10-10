//! Run on an isolated real gateway's vault host with real discovered accounts.
//! Obsolete members and their vault item IDs are explicit fixture inputs.
use crate::support::{required, Run};
use serde_json::{json, Value};

#[test]
#[ignore = "Requires isolated gateway with real owned account discovery, duplicate and identityless memberships"]
fn discovery_removes_obsolete_members_and_their_vault_items() {
    let mut run = Run::new();
    let keeper = required("RECONCILE_KEEPER");
    let duplicate = required("RECONCILE_DUPLICATE");
    let identityless = required("RECONCILE_IDENTITYLESS");
    let removed_items = required("RECONCILE_VAULT_ITEMS");
    let items: Vec<_> = removed_items.split_whitespace().collect();
    assert!(
        !items.is_empty(),
        "fixture must name actual obsolete vault items"
    );
    let kept = run.snapshot_member(&keeper);
    let duplicate_before = run.snapshot_member(&duplicate);
    let identityless_before = run.snapshot_member(&identityless);
    let account = kept["account"].as_str().expect("keeper account identity");
    assert_eq!(duplicate_before["account"], kept["account"]);
    assert_eq!(duplicate_before["provider"], kept["provider"]);
    assert!(identityless_before["account"].is_null());
    let before = run.vault_snapshot("vault-before");
    let before_rows = before.as_array().expect("vault array");
    for item in &items {
        assert!(
            before_rows
                .iter()
                .any(|row| row["id"] == *item && row["deleted"] == false),
            "fixture item is not live"
        );
    }
    let output = run.command(&["subscription", "discover"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: Value = serde_json::from_slice(&output.stdout).expect("discovery JSON");
    let removed = report["removed"].as_array().expect("removal outcomes");
    for id in [&duplicate, &identityless] {
        assert!(
            removed.iter().any(|row| row["id"] == id.as_str()),
            "obsolete member not removed"
        );
    }
    let pool = run.snapshot_pool();
    let members = pool["subscriptions"].as_array().expect("persisted pool");
    assert!(!members
        .iter()
        .any(|row| row["id"] == duplicate || row["id"] == identityless));
    let matching: Vec<_> = members
        .iter()
        .filter(|row| {
            row["provider"] == kept["provider"]
                && row["account"]
                    .as_str()
                    .is_some_and(|value| value.eq_ignore_ascii_case(account))
        })
        .collect();
    assert_eq!(
        matching
            .iter()
            .map(|row| row["id"].clone())
            .collect::<Vec<_>>(),
        vec![json!(keeper)]
    );
    let after = run.vault_snapshot("vault-after");
    let after_rows = after.as_array().expect("vault array");
    for item in &items {
        assert!(
            !after_rows
                .iter()
                .any(|row| row["id"] == *item && row["deleted"] == false),
            "obsolete vault item remains live"
        );
    }
    for row in before_rows
        .iter()
        .filter(|row| !items.iter().any(|item| row["id"] == *item))
    {
        assert!(
            after_rows.contains(row),
            "reconciliation changed an unrelated vault item"
        );
    }
    let repeated = run.command(&["subscription", "discover"]);
    assert!(
        repeated.status.success(),
        "{}",
        String::from_utf8_lossy(&repeated.stderr)
    );
    let repeat_report: Value =
        serde_json::from_slice(&repeated.stdout).expect("repeated discovery JSON");
    assert_eq!(repeat_report["removed"], json!([]));
    run.report["reconciliation"] = report;
    run.report["vault_before"] = before;
    run.report["vault_after"] = after;
    run.report["result"] = json!("passed");
    run.save();
}
