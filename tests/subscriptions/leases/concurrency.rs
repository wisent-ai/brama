//! Concurrent real CLI requests against an isolated serving gateway.
//! LEASE_HOLDERS names the concurrent callers; the journey only owns its fresh session.
use super::{required, Run};
use serde_json::{json, Value};
use std::sync::Barrier;

struct Session {
    run: Run,
    id: String,
}

impl Drop for Session {
    fn drop(&mut self) {
        let released =
            self.run
                .command(&["subscription", "lease", "release", "--session-id", &self.id]);
        self.run.report["cleanup_succeeded"] = json!(released.status.success());
        self.run.save();
    }
}

#[test]
#[ignore = "Requires isolated gateway with free LEASE_PROVIDER capacity and LEASE_HOLDERS naming concurrent callers"]
fn concurrent_calls_share_one_persisted_session_lease() {
    let provider = required("LEASE_PROVIDER");
    let holders = required("LEASE_HOLDERS");
    let mut names = holders.split_whitespace();
    assert!(names.next().is_some(), "LEASE_HOLDERS names no callers");
    assert!(
        names.next().is_some(),
        "LEASE_HOLDERS must name concurrent callers"
    );
    // Resolve and attest all destinations before any worker waits at the barrier.
    let workers: Vec<_> = holders
        .split_whitespace()
        .map(|holder| (holder, Run::new()))
        .collect();
    let mut session = Session {
        run: Run::new(),
        id: format!("lease-concurrency-{}", uuid::Uuid::new_v4()),
    };
    let barrier = Barrier::new(workers.len());
    let answers = std::thread::scope(|scope| {
        let handles: Vec<_> = workers
            .into_iter()
            .map(|(holder, mut run)| {
                let barrier = &barrier;
                let provider = &provider;
                let id = &session.id;
                scope.spawn(move || {
                    barrier.wait();
                    run.command(&[
                        "subscription",
                        "lease",
                        "take",
                        provider,
                        "--session-id",
                        id,
                        "--holder",
                        holder,
                    ])
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|handle| handle.join().expect("lease caller panicked"))
            .collect::<Vec<_>>()
    });
    let verdicts: Vec<Value> = answers
        .iter()
        .map(|answer| {
            assert!(
                answer.status.success(),
                "lease request refused: {}",
                String::from_utf8_lossy(&answer.stderr)
            );
            serde_json::from_slice(&answer.stdout).expect("lease verdict JSON")
        })
        .collect();
    let mut created = verdicts.iter().filter(|verdict| verdict["new"] == true);
    let admitted = created.next().expect("no caller created the lease");
    assert!(
        created.next().is_none(),
        "concurrent callers created duplicate leases"
    );
    for verdict in &verdicts {
        assert_eq!(verdict["lease"], admitted["lease"]);
        assert_eq!(
            verdict["live_on_subscription"],
            admitted["live_on_subscription"]
        );
        assert!(
            verdict["live_on_subscription"]
                .as_u64()
                .expect("live count")
                <= verdict["limit"].as_u64().expect("session limit")
        );
    }
    let listed = session.run.command(&["subscription", "lease", "list"]);
    assert!(listed.status.success(), "lease inventory refused");
    let inventory: Value = serde_json::from_slice(&listed.stdout).expect("lease inventory JSON");
    let mut persisted = inventory["leases"]
        .as_array()
        .expect("live leases")
        .iter()
        .filter(|lease| lease["session_id"] == session.id && lease["provider"] == provider);
    assert_eq!(
        persisted.next().expect("lease was not persisted"),
        &admitted["lease"]
    );
    assert!(
        persisted.next().is_none(),
        "register retained duplicate session leases"
    );
    let member = admitted["lease"]["subscription_id"]
        .as_str()
        .expect("subscription identity");
    let pooled = session.run.member(member, false);
    assert_eq!(pooled["live_sessions"], admitted["live_on_subscription"]);
    let released = session.run.command(&[
        "subscription",
        "lease",
        "release",
        "--session-id",
        &session.id,
    ]);
    assert!(released.status.success(), "lease release refused");
    let listed = session.run.command(&["subscription", "lease", "list"]);
    assert!(listed.status.success(), "post-release inventory refused");
    let inventory: Value =
        serde_json::from_slice(&listed.stdout).expect("post-release inventory JSON");
    assert!(!inventory["leases"]
        .as_array()
        .expect("live leases")
        .iter()
        .any(|lease| lease["session_id"] == session.id));
    session.run.report["result"] = json!("passed");
    session.run.save();
}
