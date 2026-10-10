//! A discovered real account without browser authorization support must remain unsigned.

use super::{required, Run};
use serde_json::json;

#[test]
#[ignore = "Requires a real gateway and METADATA_MEMBER discovered from a provider without Weles authorization"]
fn discovered_account_without_authorization_cannot_claim_a_grant() {
    let mut run = Run::new();
    let member = required("METADATA_MEMBER");
    let before = run.member(&member, false);
    assert_eq!(before["status"], "active");
    assert_eq!(before["retired"], false);
    assert_eq!(before["automatic_sign_in"]["applies"], false);
    assert_eq!(before["credential"]["state"], "needs_reauthorization");
    let account = before["discovery"]["account"]
        .as_str()
        .expect("persisted discovered account");
    assert_eq!(before["account"], account);
    let provider = before["provider"]
        .as_str()
        .expect("declared metadata provider");
    let response = run.command(&[
        "subscription",
        "sign-in",
        provider,
        "--by",
        "weles",
        "--subscription-id",
        &member,
        "--reason",
        "real qualification: undeclared authorization cannot mint a grant",
    ]);
    assert!(
        !response.status.success(),
        "undeclared authorization reported success"
    );
    let after = run.member(&member, false);
    assert_eq!(
        after["credential"], before["credential"],
        "refused authorization changed the independent grant state"
    );
    assert_eq!(
        after["sign_in"], before["sign_in"],
        "capability refusal fabricated a browser attempt"
    );
    assert_eq!(after["discovery"]["account"], account);
    run.report["result"] = json!("passed");
    run.save();
}
