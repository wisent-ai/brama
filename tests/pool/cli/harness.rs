//! Running the real binary against a real vault: the invocation itself, the
//! member state read back through the CLI rather than out of the ledger file,
//! and putting one retired member back in the rotation.

use std::path::Path;
use std::process::{Command, Output, Stdio};

use serde_json::Value;

use crate::support::SkarbiecVault;

/// What the pool records about the member's credential, read through the real
/// CLI rather than from the ledger file.
pub(crate) fn member_credential_state(vault: &SkarbiecVault, state: &Path) -> String {
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

pub(crate) fn reinstate(vault: &SkarbiecVault, state: &Path, subscription_id: &str) -> Output {
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
pub(crate) fn brama(vault: &SkarbiecVault, state: &Path, args: &[&str], input: Option<&[u8]>) -> Output {
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

pub(crate) fn invoke(
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
