//! An account the pool knew only through a grant borrowed from a harness
//! joins the pool as a member of its own, with the borrowed grant switched
//! off. Use an isolated real gateway whose pool holds an existing member
//! carrying `credential.borrowed_from` (never copy a harness grant to arrange
//! it) and a harness on the discovering machine that reports the same
//! account. `brama subscription discover` removes the borrowed member and
//! registers one member for the account that holds no borrowed grant, is not
//! retired, and is signed in by the gateway itself.
use super::{required, Run};
use serde_json::{json, Value};

#[test]
#[ignore = "Requires an isolated real gateway holding a borrowed member and a harness reporting the same account"]
fn a_borrowed_member_is_replaced_by_a_member_of_its_own_account() {
    let mut run = Run::new();
    let borrowed = required("BORROWED_MEMBER");
    let before = run.snapshot_member(&borrowed);
    assert!(
        before["credential"]["borrowed_from"].is_string(),
        "fixture must already hold a borrowed grant: {before}"
    );
    let provider = before["provider"]
        .as_str()
        .expect("borrowed member's provider")
        .to_owned();
    let account = before["account"]
        .as_str()
        .expect("borrowed member's account")
        .to_owned();

    let discovered = run.command(&["subscription", "discover"]);
    let report: Value = serde_json::from_slice(&discovered.stdout).expect("discovery report JSON");
    run.report["discovery"] = report.clone();
    run.save();
    assert!(
        discovered.status.success(),
        "discovery refused the account: {report}"
    );
    let removed_as_borrowed = |row: &Value| {
        row["id"] == borrowed.as_str() && row["reason"] == "borrowed grant membership"
    };
    assert!(
        report["removed"]
            .as_array()
            .expect("discovery lists what it removed")
            .iter()
            .any(removed_as_borrowed),
        "the borrowed member was not removed as a borrowed grant: {report}"
    );

    let listed = run.command(&["subscription", "list"]);
    assert!(
        listed.status.success(),
        "{}",
        String::from_utf8_lossy(&listed.stderr)
    );
    let pool: Value = serde_json::from_slice(&listed.stdout).expect("pool after discovery");
    let rows = pool["subscriptions"]
        .as_array()
        .expect("real pool subscriptions");
    assert!(
        rows.iter().all(|row| row["id"] != borrowed.as_str()),
        "the borrowed member is still in the pool"
    );
    let mut own = rows.iter().filter(|row| {
        row["provider"] == provider.as_str()
            && row["account"]
                .as_str()
                .is_some_and(|value| value.eq_ignore_ascii_case(&account))
    });
    let member = own.next().expect("the account has no member of its own");
    assert!(own.next().is_none(), "the account has more than one member");
    assert_eq!(
        member["retired"], false,
        "the account's own member is retired: {member}"
    );
    assert!(
        member["credential"]["borrowed_from"].is_null(),
        "the account's member still borrows: {member}"
    );
    assert_eq!(
        member["automatic_sign_in"]["applies"], true,
        "the gateway does not sign the account's own member in by itself: {member}"
    );
    run.report["result"] = json!("passed");
    run.save();
}
