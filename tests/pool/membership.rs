//! CLI pool membership is a real vault write, not an HTTP-only capability.
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

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

/// A retirement can be taken back, because an operator naming the accounts a
/// deployment uses is the last word on membership.
///
/// Retirement used to be permanent: `is_retired` answered yes to any
/// retirement record ever written. On 2026-09-21 all five accounts this
/// deployment's operator names as its own were retired, the gateway answered
/// `no active credential for agent` for each of them, and nothing in the
/// product could put them back. This drives the real binary: bank a member,
/// retire it, reinstate it, and read the pool the gateway itself would read.
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

/// What the pool records about the member's credential, read through the real
/// CLI rather than from the ledger file.
fn member_credential_state(vault: &SkarbiecVault, state: &Path) -> String {
    let listed = brama(vault, state, &["subscriptions", "--json"], None);
    let report: Value = serde_json::from_slice(&listed.stdout).expect("the pool answers JSON");
    report["subscriptions"]
        .as_array()
        .and_then(|rows| {
            rows.iter()
                .find(|row| row["id"] == SUBSCRIPTION)
                .and_then(|row| row["credential"]["state"].as_str())
                .map(str::to_owned)
        })
        .unwrap_or_default()
}

fn reinstate(vault: &SkarbiecVault, state: &Path, subscription_id: &str) -> Output {
    brama(
        vault,
        state,
        &[
            "subscription",
            "reinstate",
            "--subscription-id",
            subscription_id,
            "--reason",
            "the operator names this account as one this deployment uses",
        ],
        None,
    )
}

/// The real binary over this test's own vault and state, with nothing of the
/// operator's environment reachable.
fn brama(vault: &SkarbiecVault, state: &Path, args: &[&str], input: Option<&[u8]>) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_brama"));
    command
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap())
        .envs(vault.environment())
        .env("ENTITLEMENTS_ROUTER_BIN", vault.router())
        .env("BRAMA_STATE_DIR", state)
        .env(
            "BRAMA_DONATED_SUBSCRIPTIONS_FILE",
            state.join("donated.json"),
        )
        .env("BRAMA_SUBSCRIPTION_USAGE_FILE", state.join("usage.json"))
        .env("BRAMA_INFERENCE_ROUTES_FILE", state.join("routes.json"))
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().unwrap();
    let mut stdin = child.stdin.take().unwrap();
    if let Some(input) = input {
        stdin.write_all(input).unwrap();
    }
    drop(stdin);
    child.wait_with_output().unwrap()
}

fn invoke(
    vault: &SkarbiecVault,
    state: &Path,
    evidence: &Path,
    step: &str,
    input: &[u8],
) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_brama"));
    command
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap())
        .envs(vault.environment())
        .env("ENTITLEMENTS_ROUTER_BIN", vault.router())
        .env("BRAMA_STATE_DIR", state)
        .env(
            "BRAMA_DONATED_SUBSCRIPTIONS_FILE",
            state.join("donated.json"),
        )
        .env("BRAMA_SUBSCRIPTION_USAGE_FILE", state.join("usage.json"))
        .env("BRAMA_INFERENCE_ROUTES_FILE", state.join("routes.json"))
        .args(["subscriptions", "--apply", "--json"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().unwrap();
    child.stdin.take().unwrap().write_all(input).unwrap();
    let output = child.wait_with_output().unwrap();
    let file = |suffix: &str| -> PathBuf { evidence.join(format!("{step}.{suffix}")) };
    fs::write(file("stdout"), &output.stdout).unwrap();
    fs::write(file("stderr"), &output.stderr).unwrap();
    fs::write(
        file("process.json"),
        serde_json::to_vec_pretty(&json!({
            "command": [env!("CARGO_BIN_EXE_brama"), "subscriptions", "--apply", "--json"],
            "exit_code": output.status.code(), "stdin": String::from_utf8_lossy(input),
        }))
        .unwrap(),
    )
    .unwrap();
    output
}
