//! Real least-loaded allocation across an isolated provider pool, without inference.
//! LEASE_MEMBERS names every usable member; none may have a standing session.
use super::{required, Run};
use serde_json::{json, Value};
use std::collections::BTreeSet;

struct Sessions {
    run: Run,
    ids: Vec<String>,
}

impl Drop for Sessions {
    fn drop(&mut self) {
        let outcomes: Vec<_> = self
            .ids
            .iter()
            .map(|id| {
                let output =
                    self.run
                        .command(&["subscription", "lease", "release", "--session-id", id]);
                json!({"session_id": id, "released": output.status.success()})
            })
            .collect();
        self.run.report["cleanup"] = json!(outcomes);
        self.run.save();
    }
}

#[test]
#[ignore = "Requires an isolated real gateway and unused LEASE_MEMBERS for LEASE_PROVIDER"]
fn new_sessions_use_each_idle_member_before_reusing_one() {
    let provider = required("LEASE_PROVIDER");
    let declaration = brama::subscription_dispatch::acquire::declaration::provider(&provider)
        .expect("declared fixture provider");
    assert!(
        declaration.accounts_max.is_none(),
        "qualification requires a provider without authorized purchases"
    );
    let limit = brama::subscription_dispatch::acquire::declaration::sessions_cap(&provider)
        .expect("operator-stated sessions per subscription");
    let declared = required("LEASE_MEMBERS");
    let expected: BTreeSet<_> = declared.split_whitespace().map(str::to_owned).collect();
    let mut distinct = expected.iter();
    assert!(distinct.next().is_some(), "LEASE_MEMBERS names no member");
    assert!(
        distinct.next().is_some(),
        "distribution requires distinct real members"
    );
    let mut sessions = Sessions {
        run: Run::new(),
        ids: Vec::new(),
    };
    let pool = sessions.run.pool(false);
    assert_eq!(
        sessions.run.report["source_revision"],
        required("GATEWAY_SOURCE_REVISION"),
        "gateway must run this qualification source and its purchase declaration"
    );
    let available: BTreeSet<_> = pool["subscriptions"]
        .as_array()
        .expect("pool members")
        .iter()
        .filter(|row| row["provider"] == provider && row["state"] == "live")
        .map(|row| row["id"].as_str().expect("member id").to_owned())
        .collect();
    assert_eq!(
        available, expected,
        "fixture must name the whole live provider pool"
    );
    let before = sessions.run.command(&["subscription", "lease", "list"]);
    assert!(before.status.success(), "initial lease list refused");
    let before: Value = serde_json::from_slice(&before.stdout).expect("initial lease inventory");
    assert!(
        !before["leases"]
            .as_array()
            .expect("live leases")
            .iter()
            .any(|lease| lease["provider"] == provider),
        "fixture provider already carries sessions"
    );
    for _ in std::iter::repeat_n(
        (),
        usize::try_from(limit).expect("fixture capacity fits usize"),
    ) {
        let mut selected = BTreeSet::new();
        for _ in &expected {
            let id = format!("lease-distribution-{}", uuid::Uuid::new_v4());
            sessions.ids.push(id.clone());
            let output = sessions.run.command(&[
                "subscription",
                "lease",
                "take",
                &provider,
                "--session-id",
                &id,
                "--holder",
                "real-lease-distribution",
            ]);
            assert!(
                output.status.success(),
                "lease refused: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            let answer: Value = serde_json::from_slice(&output.stdout).expect("lease admission");
            assert_eq!(answer["new"], true);
            assert_eq!(answer["lease"]["session_id"], id);
            assert_eq!(answer["lease"]["provider"], provider);
            assert_eq!(answer["limit"], limit);
            assert!(answer["live_on_subscription"].as_u64().expect("live count") <= limit);
            let member = answer["lease"]["subscription_id"]
                .as_str()
                .expect("selected member");
            assert!(
                expected.contains(member),
                "lease escaped the isolated fixture"
            );
            assert!(
                selected.insert(member.to_owned()),
                "reused a member while another carried fewer sessions"
            );
            let repeated = sessions.run.command(&[
                "subscription",
                "lease",
                "take",
                &provider,
                "--session-id",
                &id,
                "--holder",
                "real-lease-distribution",
            ]);
            assert!(repeated.status.success(), "standing session lookup refused");
            let repeated: Value =
                serde_json::from_slice(&repeated.stdout).expect("standing session admission");
            assert_eq!(repeated["new"], false);
            assert_eq!(
                repeated["lease"], answer["lease"],
                "tie selection must not move a standing session to another account"
            );
            assert_eq!(
                repeated["live_on_subscription"], answer["live_on_subscription"],
                "standing session lookup must not consume another capacity slot"
            );
            let listed = sessions.run.command(&["subscription", "lease", "list"]);
            assert!(listed.status.success(), "persisted lease list refused");
            let listed: Value =
                serde_json::from_slice(&listed.stdout).expect("persisted lease inventory");
            let mut matching = listed["leases"]
                .as_array()
                .expect("persisted leases")
                .iter()
                .filter(|lease| lease["session_id"] == id && lease["provider"] == provider);
            assert_eq!(
                matching.next().expect("lease not persisted"),
                &answer["lease"]
            );
            assert!(
                matching.next().is_none(),
                "duplicate persisted session lease"
            );
            let pooled = sessions.run.member(member, false);
            assert_eq!(pooled["live_sessions"], answer["live_on_subscription"]);
            let persisted_count = listed["leases"]
                .as_array()
                .expect("persisted leases")
                .iter()
                .filter(|lease| lease["subscription_id"] == member)
                .count();
            assert_eq!(answer["live_on_subscription"], persisted_count);
        }
        assert_eq!(selected, expected);
    }
    let full = sessions.run.command(&["subscription", "lease", "list"]);
    assert!(full.status.success(), "saturated lease list refused");
    let full: Value = serde_json::from_slice(&full.stdout).expect("saturated lease inventory");
    for member in &expected {
        let count = full["leases"]
            .as_array()
            .expect("saturated leases")
            .iter()
            .filter(|lease| lease["subscription_id"] == *member)
            .count();
        assert_eq!(
            u64::try_from(count).expect("persisted count fits u64"),
            limit
        );
    }
    let overflow_id = format!("lease-overflow-{}", uuid::Uuid::new_v4());
    sessions.ids.push(overflow_id.clone());
    let overflow = sessions.run.command(&[
        "subscription",
        "lease",
        "take",
        &provider,
        "--session-id",
        &overflow_id,
        "--holder",
        "real-lease-distribution",
    ]);
    assert!(
        !overflow.status.success(),
        "full pool admitted an extra worker"
    );
    assert!(
        String::from_utf8_lossy(&overflow.stderr).contains("pool_full"),
        "overflow must refuse for capacity, not an unrelated dependency: {}",
        String::from_utf8_lossy(&overflow.stderr)
    );
    let refused = sessions.run.command(&["subscription", "lease", "list"]);
    assert!(refused.status.success(), "post-refusal lease list refused");
    let refused: Value =
        serde_json::from_slice(&refused.stdout).expect("post-refusal lease inventory");
    assert_eq!(
        refused["leases"], full["leases"],
        "overflow changed persisted leases"
    );
    for id in &sessions.ids {
        let output =
            sessions
                .run
                .command(&["subscription", "lease", "release", "--session-id", id]);
        assert!(output.status.success(), "fixture lease release refused");
    }
    let after = sessions.run.command(&["subscription", "lease", "list"]);
    assert!(after.status.success(), "post-release inventory refused");
    let after: Value = serde_json::from_slice(&after.stdout).expect("post-release inventory");
    assert!(
        !after["leases"]
            .as_array()
            .expect("live leases")
            .iter()
            .any(|lease| sessions.ids.iter().any(|id| lease["session_id"] == *id)),
        "released fixture lease remains live"
    );
    sessions.ids.clear();
    sessions.run.report["result"] = json!("passed");
    sessions.run.save();
}
