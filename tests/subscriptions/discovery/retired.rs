//! Rediscovery must not give an owner-retired account a new member identity.
//! Use an isolated real gateway with a retired legacy-named member and a harness
//! still reporting that account. This journey does not retire an operator member.
//! The text listing then counts the retired members apart from the held ones,
//! as many as the JSON listing marks retired.
use super::{required, Run};
use serde_json::{json, Value};

fn account_members(pool: &Value, provider: &str, account: &str) -> Vec<Value> {
    pool["subscriptions"]
        .as_array()
        .expect("real pool subscriptions")
        .iter()
        .filter(|row| {
            row["provider"] == provider
                && row["account"]
                    .as_str()
                    .is_some_and(|value| value.eq_ignore_ascii_case(account))
        })
        .cloned()
        .collect()
}

#[test]
#[ignore = "Requires isolated real gateway and harness reporting an owner-retired legacy member"]
fn rediscovery_cannot_register_a_retired_account_under_another_name() {
    let mut run = Run::new();
    let member = required("RETIRED_MEMBER");
    let before = run.member(&member, false);
    assert_eq!(before["retired"], true, "fixture must already be retired");
    let provider = before["provider"].as_str().expect("retired provider");
    let account = before["account"].as_str().expect("retired account address");
    let listed = run.command(&["subscription", "list"]);
    assert!(listed.status.success());
    let pool: Value = serde_json::from_slice(&listed.stdout).expect("pool before discovery");
    let members = account_members(&pool, provider, account);
    assert!(members.iter().all(|row| row["retired"] == true));
    let discovered = run.command(&["subscription", "discover"]);
    assert!(
        !discovered.status.success(),
        "retired account was rediscovered"
    );
    let report: Value = serde_json::from_slice(&discovered.stdout).expect("discovery refusal JSON");
    assert_eq!(report["ok"], false);
    assert!(report["errors"]
        .as_array()
        .expect("discovery errors")
        .iter()
        .filter_map(Value::as_str)
        .any(|error| error.contains(&member)
            && error.contains(account)
            && error.contains(provider)
            && error.contains("retired by its owner")));
    let after = run.member(&member, false);
    assert_eq!(after["retired"], true);
    assert_eq!(after["credential"], before["credential"]);
    assert_eq!(after["sign_in"], before["sign_in"]);
    let listed = run.command(&["subscription", "list"]);
    assert!(listed.status.success());
    let pool: Value = serde_json::from_slice(&listed.stdout).expect("pool after discovery");
    let after_members = account_members(&pool, provider, account);
    let mut before_ids: Vec<_> = members
        .iter()
        .map(|row| row["id"].as_str().unwrap())
        .collect();
    let mut after_ids: Vec<_> = after_members
        .iter()
        .map(|row| row["id"].as_str().unwrap())
        .collect();
    before_ids.sort_unstable();
    after_ids.sort_unstable();
    assert_eq!(
        after_ids, before_ids,
        "discovery created a new account identity"
    );
    let rows = pool["subscriptions"].as_array().expect("real pool subscriptions");
    let retired = rows
        .iter()
        .filter(|row| row["retired"] == true || row["status"] == "retired")
        .count();
    let held: Vec<&Value> = rows
        .iter()
        .filter(|row| row["retired"] != true && row["status"] != "retired")
        .collect();
    let live = held.iter().filter(|row| row["state"] == "live").count();
    let text = run.text_command(&["subscription", "list"]);
    assert!(text.status.success(), "{}", String::from_utf8_lossy(&text.stderr));
    let said = String::from_utf8_lossy(&text.stdout);
    let counted = format!(
        "{live} of {} held subscription credentials are live; {retired} retired credential(s) are listed and not counted",
        held.len()
    );
    assert!(said.lines().any(|line| line == counted), "expected `{counted}` in:\n{said}");
    run.report["result"] = json!("passed");
    run.save();
}

#[test]
#[ignore = "Requires an isolated real gateway with an occupied discovery coordinate and a harness reporting a different actual account"]
fn discovery_coordinate_collision_preserves_existing_member() {
    let mut run = Run::new();
    let member = required("COLLISION_MEMBER");
    let account = required("EXPECTED_ACCOUNT");
    let provider = required("EXPECTED_PROVIDER");
    let before = run.member(&member, false);
    assert!(
        before["discovery"].is_null(),
        "member already has discovery metadata"
    );
    assert_eq!(before["retired"], false);
    assert!(
        before["provider"] != provider
            || before["account"]
                .as_str()
                .is_none_or(|existing| !existing.eq_ignore_ascii_case(&account)),
        "the selected coordinate already belongs to the discovered account"
    );
    let discovered = run.command(&["subscription", "discover"]);
    assert!(
        !discovered.status.success(),
        "coordinate collision was admitted"
    );
    let report: Value = serde_json::from_slice(&discovered.stdout).expect("discovery refusal JSON");
    assert!(report["errors"]
        .as_array()
        .expect("discovery errors")
        .iter()
        .filter_map(Value::as_str)
        .any(|error| error.contains(&member)
            && error.contains(&account)
            && error.contains(&provider)
            && error.contains("conflicts with an existing subscription coordinate")));
    let after = run.member(&member, false);
    for field in [
        "provider",
        "account",
        "discovery",
        "credential",
        "sign_in",
        "retired",
    ] {
        assert_eq!(
            after[field], before[field],
            "discovery changed existing member {field}"
        );
    }
    run.report["result"] = json!("passed");
    run.save();
}
