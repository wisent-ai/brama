//! Real signed callers against an isolated gateway; only this journey's fresh lease is changed.
use super::{required, Run};
use serde_json::{json, Value};

struct OwnedLease {
    run: Run,
    session: String,
    agent: String,
    signing_role: String,
}

impl OwnedLease {
    fn signed(&mut self, action: &[&str]) -> std::process::Output {
        let mut arguments = action.to_vec();
        arguments.extend(["--agent", &self.agent, "--signing-role", &self.signing_role]);
        self.run.command(&arguments)
    }
}

impl Drop for OwnedLease {
    fn drop(&mut self) {
        let session = self.session.clone();
        let released = self.signed(&["subscription", "lease", "release", "--session-id", &session]);
        self.run.report["cleanup"] = json!({"exit_status": released.status.code()});
        self.run.save();
    }
}

#[test]
#[ignore = "Requires isolated gateway with free LEASE_PROVIDER capacity and two provisioned agent signing roles"]
fn signed_leases_are_visible_and_releasable_only_to_their_owner() {
    let provider = required("LEASE_PROVIDER");
    let second_agent = required("LEASE_AGENT_B");
    let second_role = required("LEASE_SIGNING_ROLE_B");
    let mut owned = OwnedLease {
        run: Run::new(),
        session: format!("lease-ownership-{}", uuid::Uuid::new_v4()),
        agent: required("LEASE_AGENT_A"),
        signing_role: required("LEASE_SIGNING_ROLE_A"),
    };
    let incomplete = owned.run.command(&["subscription", "lease", "list", "--agent", &owned.agent]);
    assert!(!incomplete.status.success(), "an unsigned agent request was accepted");
    assert!(String::from_utf8_lossy(&incomplete.stderr).contains("--signing-role"),
        "the missing signing role was not named: {}", String::from_utf8_lossy(&incomplete.stderr));
    assert_ne!(owned.agent, second_agent, "two independent identities are required");
    let session = owned.session.clone();
    let taken = owned.signed(&["subscription", "lease", "take", &provider,
        "--session-id", &session, "--holder", "lease-ownership-journey"]);
    assert!(taken.status.success(), "owner take refused: {}", String::from_utf8_lossy(&taken.stderr));
    let document: Value = serde_json::from_slice(&taken.stdout).expect("take document");
    let lease_id = document["lease"]["id"].as_str().expect("lease id").to_owned();
    assert_eq!(document["lease"]["session_id"], session);

    let listed = owned.signed(&["subscription", "lease", "list"]);
    assert!(listed.status.success(), "owner list refused: {}", String::from_utf8_lossy(&listed.stderr));
    let owner: Value = serde_json::from_slice(&listed.stdout).expect("owner list document");
    assert_eq!(owner["scope"], owned.agent);
    assert!(owner["leases"].as_array().expect("owner leases").iter().any(|lease| lease["id"] == lease_id));

    let other = owned.run.command(&["subscription", "lease", "list",
        "--agent", &second_agent, "--signing-role", &second_role]);
    assert!(other.status.success(), "other list refused: {}", String::from_utf8_lossy(&other.stderr));
    let other_list: Value = serde_json::from_slice(&other.stdout).expect("other list document");
    assert_eq!(other_list["scope"], second_agent);
    assert!(!other_list["leases"].as_array().expect("other leases").iter().any(|lease| lease["id"] == lease_id));

    let refused = owned.run.command(&["subscription", "lease", "release", "--lease-id", &lease_id,
        "--agent", &second_agent, "--signing-role", &second_role]);
    assert!(!refused.status.success(), "another agent released the lease");
    let cause = String::from_utf8_lossy(&refused.stderr);
    assert!(cause.contains("HTTP 403 at /v1/subscription-pool/leases")
        && cause.contains("may end only the leases it took")
        && cause.contains("nothing was released"),
        "foreign ownership refusal lost its operation or cause: {cause}");
    let listed = owned.signed(&["subscription", "lease", "list"]);
    assert!(listed.status.success(), "post-refusal list refused");
    let retained: Value = serde_json::from_slice(&listed.stdout).expect("retained lease list");
    assert!(retained["leases"].as_array().expect("retained leases").iter().any(|lease| lease["id"] == lease_id));

    let released = owned.signed(&["subscription", "lease", "release", "--lease-id", &lease_id]);
    assert!(released.status.success(), "owner release refused: {}", String::from_utf8_lossy(&released.stderr));
    let listed = owned.signed(&["subscription", "lease", "list"]);
    assert!(listed.status.success(), "final list refused");
    let final_state: Value = serde_json::from_slice(&listed.stdout).expect("final lease list");
    assert!(!final_state["leases"].as_array().expect("final leases").iter().any(|lease| lease["id"] == lease_id));
    owned.run.report["observed_final_state"] = final_state;
    owned.run.report["result"] = json!("passed");
    owned.run.save();
}
