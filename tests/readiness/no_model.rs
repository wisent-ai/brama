//! Qualification of the gateway with its model provider switched off: an
//! isolated gateway whose subscription pool the usage ledger refuses
//! entirely (`BRAMA_SUBSCRIPTION_CATALOG` and `BRAMA_SUBSCRIPTION_USAGE_FILE`
//! given to it at start), so no member holds a grant that can be served.
//! `/readyz` has to answer 503 with `model.state` `none`, its `model.reason`
//! naming what renews the pool, and the top-level `reason` keeping that
//! cause beside any other the report holds. One request for `best` is then
//! sent through the real `brama probe`, signed as the agent the caller
//! names; it is refused `subscription_reauthorization_required` with the
//! same sentence `/readyz` gave, because the refusal is read from the
//! ledger before any credential read or provider call.
//!
//! A second gateway, named by `BRAMA_UNREADABLE_POOL_READINESS_URL`, is
//! started with a signing-agent declaration it cannot read: its `/readyz`
//! has to answer 503 with `model.state` `unknown` and that failed read in
//! `reason`, so a client that reads readiness first sends nothing.
#[path = "../subscriptions/support.rs"]
mod support;

use serde_json::{json, Value};
use support::{required, Run};

#[tokio::test(flavor = "multi_thread")]
#[ignore = "Requires a real isolated gateway whose pool the ledger refuses entirely, a client bearer role, and an agent with its signing role"]
async fn a_pool_no_member_of_which_can_be_served_is_named_unready_and_refused_at_once() {
    let url = required("BRAMA_READINESS_URL");
    let bearer_role = required("CLIENT_BEARER_ROLE");
    let agent = required("AGENT");
    let signing_role = required("SIGNING_ROLE");

    let mut run = Run::new();
    let response = reqwest::get(&url).await.expect("readiness answers");
    let status = response.status().as_u16();
    let readiness: Value = response.json().await.expect("readiness JSON");
    run.report["readiness"] = json!({ "status": status, "body": readiness });
    run.save();
    assert_eq!(
        status,
        reqwest::StatusCode::SERVICE_UNAVAILABLE.as_u16(),
        "a gateway with no servable member answered readiness as ready: {readiness}"
    );
    assert_eq!(
        readiness["model"]["state"], "none",
        "readiness does not say there is no model: {readiness}"
    );
    let cause = readiness["model"]["reason"]
        .as_str()
        .expect("the missing model is named in words");
    let reason = readiness["reason"]
        .as_str()
        .expect("readiness states its reason");
    assert!(
        reason.contains(cause),
        "readiness's reason dropped the model cause {cause:?}: {reason}"
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
        "the request for best was not refused for want of a servable member: {answer}"
    );
    let member = cause.split(" | ").next().expect("the cause names a pool");
    assert!(
        answer.contains(member),
        "the refusal does not carry the verdict readiness gave ({member:?}): {answer}"
    );
    run.report["result"] = json!("passed");
    run.save();
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "Requires a real isolated gateway started with a signing-agent declaration it cannot read"]
async fn a_pool_that_cannot_be_read_is_named_unready() {
    let url = required("BRAMA_UNREADABLE_POOL_READINESS_URL");
    let mut run = Run::new();
    let response = reqwest::get(&url).await.expect("readiness answers");
    let status = response.status().as_u16();
    let readiness: Value = response.json().await.expect("readiness JSON");
    run.report["readiness"] = json!({ "status": status, "body": readiness });
    run.save();
    assert_eq!(
        status,
        reqwest::StatusCode::SERVICE_UNAVAILABLE.as_u16(),
        "a gateway that cannot read its pool answered readiness as ready: {readiness}"
    );
    assert_eq!(
        readiness["model"]["state"], "unknown",
        "readiness does not say the pool is unread: {readiness}"
    );
    let cause = readiness["model"]["reason"]
        .as_str()
        .expect("the failed read is named in words");
    let reason = readiness["reason"]
        .as_str()
        .expect("readiness states its reason");
    assert!(
        reason.contains(cause),
        "readiness's reason dropped the failed read {cause:?}: {reason}"
    );
    run.report["result"] = json!("passed");
    run.save();
}
