//! Qualification of the maintenance pass a refusal starts. The isolated
//! gateway runs with a subscription pool whose every member the usage ledger
//! refuses (`BRAMA_SUBSCRIPTION_CATALOG` and `BRAMA_SUBSCRIPTION_USAGE_FILE`
//! given to it at start) and with no `brama maintain` schedule, so nothing
//! but the gateway itself can run a pass. `/readyz` is read first and has
//! to say no pass has run. One request for `best` is sent through the real
//! `brama probe`, signed as the agent the caller names; it is refused
//! `subscription_reauthorization_required`. `/readyz` is then asked again
//! as each answer comes back, until it reports the pass that refusal started.
#[path = "../subscriptions/support.rs"]
mod support;

use serde_json::{json, Value};
use support::{required, Run};

async fn readiness(client: &reqwest::Client, url: &str) -> (u16, Value) {
    let response = client.get(url).send().await.expect("readiness answers");
    let status = response.status().as_u16();
    (status, response.json().await.expect("readiness JSON"))
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "Requires a real isolated gateway whose pool the ledger refuses entirely, no maintenance schedule, a client bearer role, and an agent with its signing role"]
async fn a_refusal_for_want_of_a_signed_in_subscription_starts_one_maintenance_pass() {
    let url = required("BRAMA_READINESS_URL");
    let bearer_role = required("CLIENT_BEARER_ROLE");
    let agent = required("AGENT");
    let signing_role = required("SIGNING_ROLE");
    let client = reqwest::Client::new();

    let mut run = Run::new();
    let (status, before) = readiness(&client, &url).await;
    run.report["before"] = json!({ "status": status, "body": before });
    run.save();
    assert!(
        before["maintenance"]["last_pass_unix"].is_null(),
        "the isolated gateway already ran a pass, so a refusal cannot be shown to start one: {before}"
    );
    let detail = before["maintenance"]["detail"]
        .as_str()
        .expect("no pass is said in words");
    assert!(
        detail.contains("refused for want of a signed-in subscription"),
        "readiness does not say a refusal starts a pass: {detail}"
    );

    let output = run.standalone(&[
        "probe",
        "--alias",
        "best",
        "--bearer-role",
        &bearer_role,
        "--agent",
        &agent,
        "--signing-role",
        &signing_role,
        "--allow-provider-cost",
        "--json",
    ]);
    let answer = String::from_utf8_lossy(&output.stdout).into_owned();
    run.report["probe"] = json!({
        "status": output.status.code(),
        "stdout": answer,
        "stderr": String::from_utf8_lossy(&output.stderr),
    });
    run.save();
    assert!(
        answer.contains("subscription_reauthorization_required"),
        "the request for best was not refused for want of a signed-in subscription: {answer}"
    );

    let mut readings = Vec::new();
    let after = loop {
        let (status, body) = readiness(&client, &url).await;
        readings.push(json!({ "status": status, "body": body }));
        if body["maintenance"]["last_pass_unix"].is_u64() {
            break body;
        }
    };
    run.report["readings"] = json!(readings);
    run.save();
    assert!(
        after["maintenance"]["last_pass_ok"].is_boolean(),
        "the pass is reported without how it ended: {after}"
    );
    run.report["result"] = json!("passed");
    run.save();
}
