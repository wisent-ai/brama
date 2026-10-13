//! Qualification of a gateway whose credential source accepts and never
//! answers. The isolated gateway runs with `WC_SKARBIEC_URL` naming the
//! stand-in this test listens on (`STANDIN_SKARBIEC_ADDRESS`, a loopback
//! address the fixture host handed out and passed to both), and with a
//! resolver state whose registry copy declares the refresh interval. The
//! stand-in accepts each connection, reads the request and never answers.
//! A workload admission is sent through the real CLI; while it stands,
//! `/readyz` is asked again as each answer comes back. Once the admission has
//! stood past the declared interval, readiness declares the gateway serves
//! nothing (503, `ready: false`) naming the credential source, and the same
//! reading ends the admission, which the CLI reports as 503 with that
//! sentence instead of standing on.
#[path = "../subscriptions/support.rs"]
mod support;

use std::io::Read;
use std::net::TcpListener;

use serde_json::{json, Value};
use support::{required, Run};

/// Accept every connection and read what it sends, answering nothing, for
/// as long as the test runs.
fn stand_in(address: &str) {
    let listener =
        TcpListener::bind(address).expect("the stand-in binds the address the gateway was given");
    std::thread::spawn(move || {
        let mut held = Vec::new();
        for connection in listener.incoming().flatten() {
            let mut connection = connection;
            let mut request = Vec::new();
            let _ = connection.read_to_end(&mut request);
            held.push(connection);
        }
    });
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "Requires a real isolated gateway started against this stand-in, its resolver state with a declared refresh interval, and a workload bearer role"]
async fn a_credential_source_that_never_answers_ends_readiness_and_the_standing_admission() {
    let address = required("STANDIN_SKARBIEC_ADDRESS");
    let readiness = required("BRAMA_READINESS_URL");
    let workload_role = required("WORKLOAD_BEARER_ROLE");
    let consumer = required("CONSUMER");
    stand_in(&address);

    let mut run = Run::new();
    run.report["standing"] = json!({ "stand_in": address, "readiness": readiness });
    run.save();
    let admission = std::thread::spawn(move || {
        let mut run = Run::new();
        let output = run.standalone(&[
            "subscription",
            "lease",
            "list",
            "--gateway-consumer",
            &consumer,
            "--bearer-role",
            &workload_role,
            "--json",
        ]);
        (output, run)
    });

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
        assert!(
            !admission.is_finished(),
            "the admission ended before readiness named the credential source: {readings:?}"
        );
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
        sentence.contains(&address),
        "readiness does not name the stand-in: {sentence}"
    );

    let (output, _) = admission.join().expect("the admission thread ends");
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    run.report["admission"] = json!({ "status": output.status.code(), "stderr": stderr });
    run.save();
    assert!(
        !output.status.success(),
        "a standing credential source admitted a workload"
    );
    let unavailable = format!(
        "HTTP {}:",
        reqwest::StatusCode::SERVICE_UNAVAILABLE.as_u16()
    );
    assert!(
        stderr.contains(&unavailable),
        "the admission was not refused 503: {stderr}"
    );
    run.report["result"] = json!("passed");
    run.save();
}
