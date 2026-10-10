//! Run on the isolated serving gateway's vault host, with actual accounts.
use crate::support::{required, Run};
use serde_json::{json, Value};

#[test]
#[ignore = "Requires a real fully attributed provider and a provider with an unattributed live member on the gateway vault host"]
fn authenticator_report_counts_accounts_and_refuses_missing_identity() {
    let mut run = Run::new();
    let attributed = required("ATTRIBUTED_PROVIDER");
    let unattributed = required("UNATTRIBUTED_PROVIDER");
    let member = required("UNATTRIBUTED_MEMBER");
    let vault_before = run.vault_snapshot("vault-before");
    let expected_accounts: Vec<String> = required("ATTRIBUTED_ACCOUNTS")
        .split_whitespace()
        .map(str::to_lowercase)
        .collect();
    let local = run.standalone(&["subscription", "list", "--json"]);
    let before: Value = serde_json::from_slice(&local.stdout).expect("real local pool report");
    assert_eq!(
        before["build"]["source_revision"],
        json!(required("GATEWAY_SOURCE_REVISION"))
    );
    let rows = before["subscriptions"]
        .as_array()
        .expect("real local memberships");
    assert!(
        rows.iter().any(|row| row["id"] == member
            && row["provider"] == unattributed
            && row["account"].is_null()),
        "fixture must contain the actual unattributed member"
    );
    let output = run.standalone(&[
        "subscription",
        "authenticator",
        "list",
        "--provider",
        &attributed,
        "--json",
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: Value = serde_json::from_slice(&output.stdout).expect("authenticator report");
    let mut actual: Vec<_> = report["accounts"]
        .as_array()
        .expect("attributed accounts")
        .iter()
        .map(|row| {
            row["account"]
                .as_str()
                .expect("account identity")
                .to_lowercase()
        })
        .collect();
    actual.sort();
    let mut expected_accounts = expected_accounts;
    expected_accounts.sort();
    assert_eq!(
        actual, expected_accounts,
        "authenticator report must contain actual accounts, not member IDs"
    );
    let refused = run.standalone(&[
        "subscription",
        "authenticator",
        "list",
        "--provider",
        &unattributed,
        "--json",
    ]);
    assert!(
        !refused.status.success(),
        "missing identity was counted as an account"
    );
    assert!(
        refused.stdout.is_empty(),
        "refused report printed account totals"
    );
    let cause = String::from_utf8_lossy(&refused.stderr);
    assert!(
        cause.contains(&member) && cause.contains(&unattributed),
        "refusal must identify the actual member and provider: {cause}"
    );
    let text = run.standalone(&[
        "subscription",
        "authenticator",
        "list",
        "--provider",
        &unattributed,
    ]);
    assert!(!text.status.success());
    assert!(
        text.stdout.is_empty(),
        "text refusal printed account totals"
    );
    let after = run.standalone(&["subscription", "list", "--json"]);
    let after: Value = serde_json::from_slice(&after.stdout).expect("persisted local pool report");
    assert_eq!(
        after["subscriptions"], before["subscriptions"],
        "read-only authenticator reporting changed membership or credential state"
    );
    let vault_after = run.vault_snapshot("vault-after");
    assert_eq!(
        vault_after, vault_before,
        "read-only authenticator reporting changed the real vault"
    );
    run.report["authenticator_report"] = report;
    run.report["result"] = json!("passed");
    run.save();
}
