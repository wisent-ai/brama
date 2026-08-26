//! The API-key providers this deployment holds real keys for, tested against
//! the real thing.
//!
//! An API-key provider makes exactly one claim: the key in the vault redeems
//! at final use and the provider serves a real completion for it. The product
//! path that spends the deployment's own capability from the CLI is `brama
//! onboard --allow-provider-cost`, whose completion is recorded only after a
//! real model response (`src/onboarding.rs`) -- viewing the steps or allowing
//! cost is not sufficient, so its recorded completion is the state this test
//! reads.
//!
//! The journey remembers being completed and answers early without sending a
//! request on a second run, so each test points `XDG_STATE_HOME` at a fresh
//! directory. That isolates only the journey's own bookkeeping file; the
//! vault, the capability environment and the provider are the real ones, and
//! the request is a real billable completion:
//!
//! ```console
//! $ scripts/start-with-skarbiec.sh env cargo test --test capability_real
//! ```
//!
//! Which providers appear here is not taste: `/readyz` on the deployment
//! names the providers a capability is configured for, and a test for a
//! provider this deployment holds no key for could only prove a refusal --
//! which `provider_contracts` already does for all 23.

use std::process::Command;

use serde_json::Value;

const AGENT: &str = "wisent-app";

/// One capability environment, one billable journey at a time.
static REAL_CAPABILITY: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// A fresh journey state directory: unique per process and per call, under the
/// operator-visible scratch area rather than /tmp.
fn fresh_state_dir(story: &str) -> std::path::PathBuf {
    let home = std::env::var("HOME").expect("HOME is required");
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock")
        .as_nanos();
    let path = std::path::PathBuf::from(home)
        .join(".stado/work/brama-tests")
        .join(format!("{story}-{}-{nonce}", std::process::id()));
    std::fs::create_dir_all(&path).expect("create journey state directory");
    path
}

/// One real completion paid by the deployment's capability for one provider,
/// proven by the journey recording `model_response_received` from a real
/// response: exit 0 with `--allow-provider-cost` happens on no other path.
fn capability_serves_a_real_completion(provider: &str, model_route: &str) {
    let _capability = REAL_CAPABILITY.lock().expect("real-capability lock");
    let state_dir = fresh_state_dir(&format!("onboard-{provider}"));

    let output = Command::new(env!("CARGO_BIN_EXE_brama"))
        .env("XDG_STATE_HOME", &state_dir)
        .args([
            "onboard",
            "--model",
            model_route,
            "--agent-id",
            AGENT,
            "--allow-provider-cost",
        ])
        .output()
        .expect("brama onboard");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "the {provider} capability did not serve a real completion: {stdout}{stderr}"
    );
    // The printed contract of a real served response.
    assert!(stdout.contains("Model:"), "{stdout}");
    assert!(stdout.contains("Response:"), "{stdout}");
    assert!(stdout.contains("Tokens:"), "{stdout}");
    assert!(
        stdout.contains(
            "First-use complete: Brama observed model_response_received from the real response"
        ),
        "{stdout}"
    );

    // The recorded state: the journey file this run owns marks the attempt
    // completed, which src/onboarding.rs writes only after a successful real
    // model response.
    let state = std::fs::read_to_string(state_dir.join("brama/onboarding.json"))
        .expect("the journey must persist its state");
    let state: Value = serde_json::from_str(&state).expect("journey state is JSON");
    assert!(
        state.to_string().contains("completed"),
        "the journey state records no completion: {state}"
    );

    let _ = std::fs::remove_dir_all(&state_dir);
}

#[test]
fn the_openai_capability_serves_a_real_completion() {
    capability_serves_a_real_completion("openai", "openai/default");
}

#[test]
fn the_featherless_capability_serves_a_real_completion() {
    capability_serves_a_real_completion("featherless", "featherless/TheDrummer/Cydonia-24B-v4.3");
}
