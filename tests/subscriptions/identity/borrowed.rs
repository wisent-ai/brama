//! Existing legacy fixture only: never copy a harness grant to arrange this test.
use crate::support::{required, Run};
use serde_json::json;

#[test]
#[ignore = "Requires an isolated real gateway with an existing marked harness grant and its owning vault host"]
fn borrowed_grant_is_refused_before_usage_refresh() {
    let mut run = Run::new();
    let member = required("BORROWED_MEMBER");
    let provider = required("EXPECTED_PROVIDER");
    let account = required("EXPECTED_ACCOUNT");
    let before = run.snapshot_member(&member);
    assert_eq!(before["provider"], provider);
    assert_eq!(before["account"], account);
    assert!(
        before["credential"]["borrowed_from"].is_string(),
        "fixture must already retain legacy grant provenance"
    );
    let vault_before = run.vault_snapshot("vault-before");
    let refused = run.refused_member(&member);
    let cause = refused["last_redeem_error"]
        .as_str()
        .expect("persisted grant refusal");
    assert!(cause.contains("borrowed harness grant"), "{cause}");
    assert!(
        cause.contains(&member),
        "refusal must name the actual member: {cause}"
    );
    assert_eq!(refused["account"], before["account"]);
    assert_eq!(refused["provider"], before["provider"]);
    assert_eq!(
        refused["credential"]["borrowed_from"],
        before["credential"]["borrowed_from"]
    );
    assert_eq!(refused["reset_redemption"], before["reset_redemption"]);
    let persisted = run.snapshot_member(&member);
    assert_eq!(persisted["last_redeem_error"], refused["last_redeem_error"]);
    assert_eq!(persisted["credential"], refused["credential"]);
    let vault_after = run.vault_snapshot("vault-after");
    assert_eq!(
        vault_after, vault_before,
        "refusal must preserve the real vault inventory"
    );
    run.report["refusal"] = refused;
    run.report["result"] = json!("passed");
    run.save();
}
