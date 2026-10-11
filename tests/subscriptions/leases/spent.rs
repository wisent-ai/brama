//! A session is never leased onto a spent plan. LEASE_SPENT_PROVIDER names a
//! declared provider without authorized purchases whose every pool member
//! has a current plan window at its whole limit, as the gateway's own pool
//! document reports it. A lease for it is refused `no_usable_member`, each
//! member is named with the window at its limit, the refusal carries the
//! acquisition it started, and the lease register is unchanged.
use super::{required, Run};
use serde_json::{json, Value};

#[test]
#[ignore = "Requires a real gateway whose LEASE_SPENT_PROVIDER members are all at a plan limit"]
fn a_pool_whose_every_plan_is_at_its_limit_leases_nothing_and_starts_an_acquisition() {
    let provider = required("LEASE_SPENT_PROVIDER");
    let declaration = brama::subscription_dispatch::acquire::declaration::provider(&provider)
        .expect("declared fixture provider");
    assert!(
        declaration.accounts_max.is_none(),
        "the journey reads a refused acquisition, so the provider authorizes no purchase"
    );
    let mut run = Run::new();
    let pool = run.pool(true);
    let members: Vec<String> = pool["subscriptions"]
        .as_array()
        .expect("pool members")
        .iter()
        .filter(|row| row["provider"] == provider)
        .map(|row| row["id"].as_str().expect("member id").to_owned())
        .collect();
    assert!(
        !members.is_empty(),
        "the gateway's pool holds no {provider} member"
    );
    let before = run.command(&["subscription", "lease", "list"]);
    assert!(before.status.success(), "initial lease list refused");
    let before: Value = serde_json::from_slice(&before.stdout).expect("initial lease inventory");

    let session_id = format!("lease-spent-{}", uuid::Uuid::new_v4());
    let refused = run.command(&[
        "subscription",
        "lease",
        "take",
        &provider,
        "--session-id",
        &session_id,
        "--holder",
        "real-lease-spent-pool",
    ]);
    let said = String::from_utf8_lossy(&refused.stderr).into_owned();
    run.report["refusal"] = json!(said.trim());
    run.save();
    if refused.status.success() {
        let released = run.command(&[
            "subscription",
            "lease",
            "release",
            "--session-id",
            &session_id,
        ]);
        run.report["cleanup_released"] = json!(released.status.success());
        run.save();
        panic!("a pool whose every plan is at its limit gave a lease");
    }
    assert!(
        said.contains("no_usable_member"),
        "a spent pool is refused as having no usable member, not as full or unavailable: {said}"
    );
    for member in &members {
        assert!(
            said.contains(member.as_str()),
            "the refusal names member {member}: {said}"
        );
    }
    assert!(
        said.contains("is at its whole limit"),
        "each member is named with its plan window at the limit: {said}"
    );
    assert!(
        said.contains("acquisition") && said.contains("no_usable_member"),
        "the refusal carries the acquisition started for the spent pool: {said}"
    );

    let after = run.command(&["subscription", "lease", "list"]);
    assert!(after.status.success(), "post-refusal lease list refused");
    let after: Value = serde_json::from_slice(&after.stdout).expect("post-refusal lease inventory");
    assert_eq!(
        before["leases"], after["leases"],
        "a refused lease changed the register"
    );
    run.report["result"] = json!("passed");
    run.save();
}
