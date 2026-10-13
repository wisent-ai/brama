//! Qualification of a gateway whose vault executable accepts every command
//! and never answers. The isolated gateway is started with `SKARBIEC_BIN`
//! naming this test's own executable and `BRAMA_TEST_VAULT_STANDS` set in
//! its environment: run that way, this executable is the vault, reads no
//! input and never exits, so every vault child the gateway shells — the
//! readiness sweep's own `list` first — stands. The gateway also runs with a
//! resolver state whose registry copy declares the refresh interval. With
//! `BRAMA_READINESS_URL` naming that gateway, `/readyz` is asked again as
//! each answer comes back; once a vault child has stood past the declared
//! interval, readiness declares the gateway serves nothing (503, `ready:
//! false`) and its sentence names the vault child, the operation and its
//! pid, so the stall is read from Brama instead of from a client's empty
//! reply minutes later. The reading also ends the child, whose end the
//! gateway's log names as `vault child abandoned by the readiness reading`.
//!
//! This binary carries no test harness, because the harness would read the
//! vault's command words as its own options: `main` is the vault when
//! `BRAMA_TEST_VAULT_STANDS` is set, the journey when run with `--ignored`,
//! and otherwise says it is ignored and why, as a harness would. Run:
//! `cargo test --test readiness-vault-standing -- --ignored` against the
//! gateway so started.
#[path = "../subscriptions/support.rs"]
mod support;

use serde_json::{json, Value};
use support::{required, Run};

/// The name under which this executable is told it is the vault.
const STANDS_ENV: &str = "BRAMA_TEST_VAULT_STANDS";

/// Whether this process was started as the vault that never answers.
fn runs_as_the_vault() -> bool {
    std::env::var_os(STANDS_ENV).is_some_and(|value| !value.is_empty())
}

fn main() {
    if runs_as_the_vault() {
        // The gateway's child: stand until it is ended.
        loop {
            std::thread::park();
        }
    }
    if !std::env::args().any(|argument| argument == "--ignored") {
        println!(
            "ignored: a_vault_child_that_never_answers_ends_readiness_naming_it requires a real \
             isolated gateway started with SKARBIEC_BIN naming this executable, {STANDS_ENV} set, \
             and its resolver state with a declared refresh interval; run with -- --ignored"
        );
        return;
    }
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("a runtime for the journey")
        .block_on(a_vault_child_that_never_answers_ends_readiness_naming_it());
}

async fn a_vault_child_that_never_answers_ends_readiness_naming_it() {
    let readiness = required("BRAMA_READINESS_URL");
    let mut run = Run::new();
    run.report["vault_standing"] = json!({
        "readiness": readiness,
        "vault": std::env::current_exe().expect("this executable's path"),
    });
    run.save();

    let client = reqwest::Client::new();
    let mut readings = Vec::new();
    let declared = loop {
        let response = client
            .get(&readiness)
            .send()
            .await
            .expect("readiness answers");
        let status = response.status();
        let body: Value = response.json().await.expect("readiness JSON");
        readings.push(json!({ "status": status.as_u16(), "body": body }));
        if body["credential_source"].is_string() {
            break (status, body);
        }
    };
    run.report["readings"] = json!(readings);
    run.save();
    let (status, body) = declared;
    assert_eq!(status, reqwest::StatusCode::SERVICE_UNAVAILABLE, "{body}");
    assert_eq!(body["ready"], false, "{body}");
    let sentence = body["credential_source"]
        .as_str()
        .expect("the cause is a sentence");
    assert!(
        sentence.contains("the credential source has not answered"),
        "{sentence}"
    );
    assert!(
        sentence.contains("vault child `") && sentence.contains("(pid "),
        "readiness does not name the standing vault child and its pid: {sentence}"
    );
    run.report["result"] = json!("passed");
    run.save();
    println!("passed: {sentence}");
}
