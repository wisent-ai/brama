//! CLI pool membership is a real vault write, not an HTTP-only capability.
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use serde_json::{json, Value};
use crate::support::{SkarbiecVault, TestDirectory};

const AGENT: &str = "pool-cli-test";
const PROVIDER: &str = "openai";
const SUBSCRIPTION: &str = "brama-sub-pool-cli-test-openai-primary";

#[test]
fn cli_banks_and_retires_membership_and_refuses_invalid_writes() {
    let vault = SkarbiecVault::create("cli-bank");
    let state = TestDirectory::new("cli-state");
    let evidence = Path::new(env!("CARGO_MANIFEST_DIR")).join(".wisent-output/pool")
        .join(state.path().file_name().unwrap());
    fs::create_dir_all(&evidence).unwrap();
    let revision = Command::new("git").args(["rev-parse", "HEAD"])
        .current_dir(env!("CARGO_MANIFEST_DIR")).output().unwrap();
    assert!(revision.status.success());
    fs::write(evidence.join("source-revision.txt"), revision.stdout).unwrap();
    let bank = json!({"action": "bank", "agent_id": AGENT, "provider": PROVIDER,
        "api_key": "isolated-membership-test-value", "label": "CLI membership"});
    let banked = invoke(&vault, state.path(), &evidence, "bank", bank.to_string().as_bytes());
    assert!(banked.status.success(), "{}", String::from_utf8_lossy(&banked.stdout));
    let receipt: Value = serde_json::from_slice(&banked.stdout).unwrap();
    assert_eq!(receipt["subscription"]["id"], SUBSCRIPTION);
    let item = SkarbiecVault::item_id(PROVIDER, SUBSCRIPTION);
    let tags = vault.tags_of(&item);
    assert!(tags.contains(&format!("brama:agent:{AGENT}")), "{tags:?}");
    assert!(tags.contains(&format!("brama:id:{SUBSCRIPTION}")), "{tags:?}");
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
    let retired = invoke(&vault, state.path(), &evidence, "retire", retire.to_string().as_bytes());
    assert!(retired.status.success(), "{}", String::from_utf8_lossy(&retired.stdout));
    assert_eq!(vault.list(), members, "retirement must not delete the stored credential");
    let usage: Value = serde_json::from_slice(&fs::read(state.path().join("usage.json")).unwrap()).unwrap();
    assert_eq!(usage["subscriptions"][SUBSCRIPTION]["credential"]["state"], "disabled");
    let journal = fs::read_to_string(state.path().join("journal.jsonl")).unwrap();
    assert!(journal.lines().any(|line| {
        let event: Value = serde_json::from_str(line).unwrap();
        event["kind"] == "retire" && event["id"] == SUBSCRIPTION
    }));
    fs::write(evidence.join("retired-journal.jsonl"), journal).unwrap();
    fs::copy(&overlay_path, evidence.join("retired-membership.json")).unwrap();
    eprintln!("CLI membership evidence: {}", evidence.display());
}

fn invoke(vault: &SkarbiecVault, state: &Path, evidence: &Path, step: &str, input: &[u8]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_brama"));
    command.env_clear().env("PATH", std::env::var_os("PATH").unwrap())
        .envs(vault.environment()).env("ENTITLEMENTS_ROUTER_BIN", vault.router())
        .env("BRAMA_STATE_DIR", state).env("BRAMA_DONATED_SUBSCRIPTIONS_FILE", state.join("donated.json"))
        .env("BRAMA_SUBSCRIPTION_USAGE_FILE", state.join("usage.json"))
        .env("BRAMA_INFERENCE_ROUTES_FILE", state.join("routes.json"))
        .args(["subscriptions", "--apply", "--json"])
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = command.spawn().unwrap();
    child.stdin.take().unwrap().write_all(input).unwrap();
    let output = child.wait_with_output().unwrap();
    let file = |suffix: &str| -> PathBuf { evidence.join(format!("{step}.{suffix}")) };
    fs::write(file("stdout"), &output.stdout).unwrap();
    fs::write(file("stderr"), &output.stderr).unwrap();
    fs::write(file("process.json"), serde_json::to_vec_pretty(&json!({
        "command": [env!("CARGO_BIN_EXE_brama"), "subscriptions", "--apply", "--json"],
        "exit_code": output.status.code(), "stdin": String::from_utf8_lossy(input),
    })).unwrap()).unwrap();
    output
}
