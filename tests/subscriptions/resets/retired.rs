//! Exercise reset refusal for an actually retired gateway member without changing its state.

use super::{required, Run};
use serde_json::json;

#[test]
#[ignore = "Requires a real gateway and an owner-retired subscription selected by RETIRED_MEMBER"]
fn retired_member_cannot_consume_a_reset() {
    let mut run = Run::new();
    let member = required("RETIRED_MEMBER");
    let before = run.member(&member, false);
    assert_eq!(before["retired"], true, "selected member is not retired");
    let provider = before["provider"]
        .as_str()
        .expect("retired member provider");
    let response = run.command(&[
        "subscription",
        "reset",
        provider,
        "--member",
        &member,
        "--reason",
        "real qualification: retired accounts must refuse reset consumption",
    ]);
    assert!(
        !response.status.success(),
        "retired member reset was admitted"
    );
    let refusal = format!(
        "{}{}",
        String::from_utf8_lossy(&response.stdout),
        String::from_utf8_lossy(&response.stderr)
    );
    assert!(
        refusal.contains(&member) && refusal.contains("retired"),
        "refusal did not identify the owner-retired member: {refusal}"
    );
    let after = run.member(&member, false);
    assert_eq!(after["retired"], true);
    assert_eq!(
        after["reset_redemption"], before["reset_redemption"],
        "retired reset refusal wrote a consumption attempt"
    );
    assert_eq!(
        after["resets"], before["resets"],
        "retired manual action overwrote the retained saved-reset report"
    );
    run.report["result"] = json!("passed");
    run.save();
}
