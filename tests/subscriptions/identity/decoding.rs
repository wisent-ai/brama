//! Existing isolated malformed credential only; never replace an operator grant.
use crate::support::{required, Run};
use serde_json::{json, Value};

#[test]
#[ignore = "Requires a real isolated gateway whose selected provider has only the malformed credential fixture"]
fn credential_decode_refusal_retains_operation_and_cause() {
    let mut run = Run::new();
    let member = required("UNREADABLE_MEMBER");
    let provider = required("EXPECTED_PROVIDER");
    let account = required("EXPECTED_ACCOUNT");
    let operation = required("EXPECTED_DECODE_OPERATION");
    let cause = required("EXPECTED_DECODE_CAUSE");
    let pool = run.snapshot_pool();
    let mut members = pool["subscriptions"]
        .as_array()
        .expect("actual pool inventory")
        .iter()
        .filter(|row| row["provider"] == provider);
    let before = members.next().expect("selected provider fixture");
    assert_eq!(before["id"], member);
    assert_eq!(before["account"], account);
    assert_eq!(before["status"], "active");
    assert_ne!(before["retired"], true);
    assert!(
        members.next().is_none(),
        "provider-wide refresh requires an isolated pool with no other member for this provider"
    );
    let vault_before = run.vault_snapshot("vault-before");
    let output = run.command(&[
        "subscription",
        "refresh",
        &provider,
        "--reason",
        "qualify malformed stored credential diagnostics without provider mutation",
    ]);
    assert!(
        !output.status.success(),
        "unreadable credential was accepted"
    );
    let refusal: Value = serde_json::from_slice(&output.stdout).expect("actual refresh verdict");
    assert_eq!(refusal["result"], "failed");
    assert!(refusal["journal_failure"].is_null(), "{refusal}");
    let detail = refusal["detail"].as_str().expect("actual refresh refusal");
    assert!(detail.contains(&member), "{detail}");
    assert!(detail.contains(&provider), "{detail}");
    assert!(detail.contains(&operation), "{detail}");
    assert!(detail.contains(&cause), "{detail}");
    let after = run.snapshot_member(&member);
    assert_eq!(after["account"], before["account"]);
    assert_eq!(after["provider"], before["provider"]);
    assert_eq!(after["reset_redemption"], before["reset_redemption"]);
    let vault_after = run.vault_snapshot("vault-after");
    assert_eq!(
        vault_after, vault_before,
        "vault inventory must remain unchanged"
    );
    run.report["refusal"] = refusal;
    run.report["result"] = json!("passed");
    run.save();
}
