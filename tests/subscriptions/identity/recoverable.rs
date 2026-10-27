//! A vault item that names an exact account but lost its `brama:subscription`
//! mark is admitted by the sign-in: the mark is restored, Weles signs the
//! account in from its own login row, and the member enters the serving pool
//! with its independent grant. Real gateway, real vault, real Weles on the
//! host Stado names; the fixture names an isolated member whose item carries
//! `brama:id:`, `brama:provider:` and `brama:account:` and no mark.
use super::{required, Run};
use serde_json::{json, Value};

const MARK: &str = "brama:subscription";

fn item<'a>(vault: &'a Value, item_id: &str) -> &'a Value {
    let Some(row) = vault
        .as_array()
        .expect("real vault inventory is a list")
        .iter()
        .find(|row| row["id"] == item_id && row["deleted"] != true)
    else {
        panic!("vault item {item_id} is absent from the real inventory");
    };
    row
}

fn tags(row: &Value) -> Vec<&str> {
    row["tags"]
        .as_array()
        .expect("vault item tags")
        .iter()
        .filter_map(Value::as_str)
        .collect()
}

#[test]
#[ignore = "Requires an isolated real gateway whose RECOVERABLE_ITEM identifies RECOVERABLE_MEMBER without the routing mark, and that account's Weles login"]
fn a_recoverable_account_is_admitted_by_its_sign_in_and_enters_the_pool() {
    let mut run = Run::new();
    let member = required("RECOVERABLE_MEMBER");
    let item_id = required("RECOVERABLE_ITEM");
    let account = required("EXPECTED_ACCOUNT");
    let provider = required("EXPECTED_PROVIDER");

    // Before: the item identifies the account and nothing routes to it.
    let before = run.vault_snapshot("vault_before");
    let row = item(&before, &item_id);
    let declared = tags(row);
    assert!(
        declared.contains(&format!("brama:id:{member}").as_str()),
        "{row}"
    );
    assert!(
        declared
            .iter()
            .any(|tag| tag.starts_with("brama:provider:")),
        "{row}"
    );
    assert!(
        declared.iter().any(|tag| {
            tag.strip_prefix("brama:account:")
                .is_some_and(|value| value.eq_ignore_ascii_case(&account))
        }),
        "{row}"
    );
    assert!(
        !declared.contains(&MARK),
        "the fixture item already carries the mark: {row}"
    );
    let pool = run.snapshot_pool();
    assert!(
        !pool["subscriptions"]
            .as_array()
            .expect("real pool subscriptions")
            .iter()
            .any(|row| row["id"] == member.as_str()),
        "the recoverable member is already in the pool: {pool}"
    );

    // The sign-in admits it: the mark is restored, Weles signs the account
    // in and the grant is banked on the item.
    let reason = "recoverable account journey: admit the item and bank its own grant";
    let signed = run.command(&[
        "subscription",
        "login",
        &provider,
        "--by",
        "weles",
        "--subscription-id",
        &member,
        "--reason",
        reason,
    ]);
    assert!(
        signed.status.success(),
        "sign-in refused: {} {}",
        String::from_utf8_lossy(&signed.stdout),
        String::from_utf8_lossy(&signed.stderr)
    );
    let verdict: Value = serde_json::from_slice(&signed.stdout).expect("sign-in verdict JSON");
    assert_eq!(verdict["result"], "signed_in", "{verdict}");
    assert_eq!(verdict["subscription_id"], member.as_str(), "{verdict}");
    assert!(
        verdict["account"]
            .as_str()
            .is_some_and(|value| value.eq_ignore_ascii_case(&account)),
        "{verdict}"
    );
    run.report["verdict"] = verdict.clone();

    // After: the item carries the mark and the member serves with its own
    // grant, attributed to the expected account.
    let after = run.vault_snapshot("vault_after");
    assert!(
        tags(item(&after, &item_id)).contains(&MARK),
        "{}",
        item(&after, &item_id)
    );
    let admitted = run.member(&member, false);
    assert_eq!(admitted["credential"]["state"], "active", "{admitted}");
    assert_eq!(admitted["sign_in"]["result"], "signed_in", "{admitted}");
    assert!(
        admitted["account"]
            .as_str()
            .is_some_and(|value| value.eq_ignore_ascii_case(&account)),
        "{admitted}"
    );
    run.report["result"] = json!("passed");
    run.save();
}
