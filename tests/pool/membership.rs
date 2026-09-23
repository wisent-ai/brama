//! CLI pool membership is a real vault write, not an HTTP-only capability.
use std::fs;
use std::path::Path;
use std::process::Command;

use crate::support::{SkarbiecVault, TestDirectory};
use serde_json::{json, Value};

const AGENT: &str = "pool-cli-test";
const PROVIDER: &str = "openai";
const SUBSCRIPTION: &str = "brama-sub-pool-cli-test-openai-primary";

#[test]
fn cli_banks_and_retires_membership_and_refuses_invalid_writes() {
    let vault = SkarbiecVault::create("cli-bank");
    let state = TestDirectory::new("cli-state");
    let evidence = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(".wisent-output/pool")
        .join(state.path().file_name().unwrap());
    fs::create_dir_all(&evidence).unwrap();
    // The revision the way src/release/build.sh states it: the release worker
    // builds an extracted source tree with no .git and names the commit in
    // BRAMA_SOURCE_REVISION; a checkout has git and nothing else.
    let revision = std::env::var("BRAMA_SOURCE_REVISION")
        .or_else(|_| std::env::var("WISENT_SOURCE_COMMIT"))
        .unwrap_or_else(|_| {
            let output = Command::new("git")
                .args(["rev-parse", "HEAD"])
                .current_dir(env!("CARGO_MANIFEST_DIR"))
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "no BRAMA_SOURCE_REVISION and no git checkout"
            );
            String::from_utf8_lossy(&output.stdout).into_owned()
        });
    fs::write(evidence.join("source-revision.txt"), revision).unwrap();
    let bank = json!({"action": "bank", "agent_id": AGENT, "provider": PROVIDER,
        "api_key": "isolated-membership-test-value", "label": "CLI membership"});
    let banked = invoke(
        &vault,
        state.path(),
        &evidence,
        "bank",
        bank.to_string().as_bytes(),
    );
    assert!(
        banked.status.success(),
        "{}",
        String::from_utf8_lossy(&banked.stdout)
    );
    let receipt: Value = serde_json::from_slice(&banked.stdout).unwrap();
    assert_eq!(receipt["subscription"]["id"], SUBSCRIPTION);
    let item = SkarbiecVault::item_id(PROVIDER, SUBSCRIPTION);
    let tags = vault.tags_of(&item);
    assert!(tags.contains(&format!("brama:agent:{AGENT}")), "{tags:?}");
    assert!(
        tags.contains(&format!("brama:id:{SUBSCRIPTION}")),
        "{tags:?}"
    );
    let overlay_path = state.path().join("donated.json");
    let overlay = fs::read(&overlay_path).expect("the CLI persisted pool membership");
    fs::write(evidence.join("banked-membership.json"), &overlay).unwrap();
    assert!(String::from_utf8_lossy(&overlay).contains(SUBSCRIPTION));
    let members = vault.list();

    for (step, request, sentence) in [
        ("invalid-json", "{broken".to_string(), "invalid subscription request:"),
        ("missing-owner", json!({"action": "bank", "provider": PROVIDER}).to_string(),
            "agent_id names the agent whose pool is written and is required for a deployment-scoped write"),
        ("unknown-action", json!({"action": "borrow", "agent_id": AGENT}).to_string(),
            "action must be \"bank\" or \"retire\""),
        ("other-owner", json!({"action": "retire", "agent_id": "not-the-owner", "subscription_id": SUBSCRIPTION}).to_string(),
            "subscription not found"),
    ] {
        let refused = invoke(&vault, state.path(), &evidence, step, request.as_bytes());
        assert_eq!(refused.status.code(), Some(1));
        let error: Value = serde_json::from_slice(&refused.stdout).unwrap();
        assert!(error["error"]["message"].as_str().unwrap().starts_with(sentence), "{error}");
        assert_eq!(fs::read(&overlay_path).unwrap(), overlay, "a refusal changed membership");
        assert_eq!(vault.list(), members, "a refusal changed the vault");
    }

    let retire = json!({"action": "retire", "agent_id": AGENT, "subscription_id": SUBSCRIPTION});
    let retired = invoke(
        &vault,
        state.path(),
        &evidence,
        "retire",
        retire.to_string().as_bytes(),
    );
    assert!(
        retired.status.success(),
        "{}",
        String::from_utf8_lossy(&retired.stdout)
    );
    assert_eq!(
        vault.list(),
        members,
        "retirement must not delete the stored credential"
    );
    let usage: Value =
        serde_json::from_slice(&fs::read(state.path().join("usage.json")).unwrap()).unwrap();
    assert_eq!(
        usage["subscriptions"][SUBSCRIPTION]["credential"]["state"],
        "disabled"
    );
    let journal = fs::read_to_string(state.path().join("journal.jsonl")).unwrap();
    assert!(journal.lines().any(|line| {
        let event: Value = serde_json::from_str(line).unwrap();
        event["kind"] == "retire" && event["id"] == SUBSCRIPTION
    }));
    fs::write(evidence.join("retired-journal.jsonl"), journal).unwrap();
    fs::copy(&overlay_path, evidence.join("retired-membership.json")).unwrap();
    eprintln!("CLI membership evidence: {}", evidence.display());
}

/// A retirement can be taken back, because naming the accounts a deployment
/// uses is the last word on membership.
///
/// Retirement used to be permanent: `is_retired` answered yes to any
/// retirement record ever written, so a member given back was answered `no
/// active credential for agent` forever and nothing could put it back. This
/// drives the real binary: bank a member, retire it, reinstate it, and read
/// the pool the gateway itself would read.
#[test]
fn the_cli_reinstates_a_retired_member_and_refuses_one_that_is_not_retired() {
    let vault = SkarbiecVault::create("cli-reinstate");
    let state = TestDirectory::new("cli-reinstate-state");
    let bank = json!({"action": "bank", "agent_id": AGENT, "provider": PROVIDER,
        "api_key": "isolated-reinstatement-test-value", "label": "CLI reinstatement"});
    let banked = invoke(
        &vault,
        state.path(),
        state.path(),
        "bank",
        bank.to_string().as_bytes(),
    );
    assert!(
        banked.status.success(),
        "{}",
        String::from_utf8_lossy(&banked.stdout)
    );

    let not_retired = reinstate(&vault, state.path(), SUBSCRIPTION);
    assert_eq!(not_retired.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&not_retired.stderr).contains("is not retired"),
        "a member in the rotation cannot be reinstated: {}",
        String::from_utf8_lossy(&not_retired.stderr)
    );

    let retire = json!({"action": "retire", "agent_id": AGENT, "subscription_id": SUBSCRIPTION});
    let retired = invoke(
        &vault,
        state.path(),
        state.path(),
        "retire",
        retire.to_string().as_bytes(),
    );
    assert!(retired.status.success());
    assert_eq!(
        member_credential_state(&vault, state.path()),
        "disabled",
        "a retirement records the member's credential as disabled"
    );

    let reinstated = reinstate(&vault, state.path(), SUBSCRIPTION);
    assert!(
        reinstated.status.success(),
        "{}{}",
        String::from_utf8_lossy(&reinstated.stdout),
        String::from_utf8_lossy(&reinstated.stderr)
    );
    let journal = fs::read_to_string(state.path().join("journal.jsonl")).unwrap();
    let decisions: Vec<Value> = journal
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|event| event["id"] == SUBSCRIPTION)
        .filter(|event| event["kind"] == "retire" || event["kind"] == "reinstate")
        .collect();
    let newest = decisions
        .last()
        .expect("the journal records the membership decisions");
    assert_eq!(newest["kind"], "reinstate", "{journal}");
    // The member is a candidate again and still holds no grant of this
    // gateway's own, which is what `needs_reauthorization` says: a
    // reinstatement is membership, not a credential.
    assert_eq!(
        member_credential_state(&vault, state.path()),
        "needs_reauthorization",
        "a reinstated member is still recorded as disabled: {journal}"
    );

    let unknown = reinstate(&vault, state.path(), "brama-sub-nobody-declared-this");
    assert_eq!(unknown.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&unknown.stderr).contains("holds no member"),
        "{}",
        String::from_utf8_lossy(&unknown.stderr)
    );
}

/// Asked how many of its accounts need a second factor, the product answers
/// from what it observed, and says so where it observed nothing.
///
/// The vault records whether a seed is stored; the provider's requirement is
/// learned only by trying to sign in. Neither was reported beside the other,
/// so the question had no answer at all. An account with no seed and no
/// attempt is not an account without a second factor, and this case holds
/// the report to saying that rather than counting it either way.
#[test]
fn the_cli_reports_the_second_factor_of_every_account_and_admits_what_it_has_not_seen() {
    let vault = SkarbiecVault::create("cli-second-factor");
    let state = TestDirectory::new("cli-second-factor-state");
    let bank = json!({"action": "bank", "agent_id": AGENT, "provider": PROVIDER,
        "api_key": "isolated-second-factor-test-value", "label": "CLI second factor"});
    let banked = invoke(
        &vault,
        state.path(),
        state.path(),
        "bank",
        bank.to_string().as_bytes(),
    );
    assert!(banked.status.success());

    let reported = brama(
        &vault,
        state.path(),
        &["subscription", "second-factor", "--json"],
        None,
    );
    assert!(
        reported.status.success(),
        "{}{}",
        String::from_utf8_lossy(&reported.stdout),
        String::from_utf8_lossy(&reported.stderr)
    );
    let report: Value = serde_json::from_slice(&reported.stdout).expect("the report is JSON");
    assert_eq!(report["required"], 0, "{report}");
    assert_eq!(report["not_required"], 0, "{report}");
    assert_eq!(report["unknown"], 1, "{report}");
    let account = report["accounts"]
        .as_array()
        .and_then(|rows| rows.first())
        .expect("the banked member is reported");
    assert_eq!(account["member"], SUBSCRIPTION, "{report}");
    assert!(
        account["required"].is_null(),
        "a member nobody signed in states no requirement: {report}"
    );
    assert!(
        account["evidence"].is_null(),
        "no observation, no evidence: {report}"
    );
    assert_eq!(
        account["seed"], "no_login_declared",
        "a member that names no login has no seed row to read: {report}"
    );
}

#[path = "cli/harness.rs"]
mod harness;

use harness::{brama, invoke, member_credential_state, reinstate};
