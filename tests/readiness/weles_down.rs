//! Qualification of the gateway with Weles switched off. The isolated gateway
//! runs with a subscription pool whose every member the usage ledger refuses
//! (`BRAMA_SUBSCRIPTION_CATALOG` and `BRAMA_SUBSCRIPTION_USAGE_FILE` given to
//! it at start) and with `BRAMA_WELES_URL` naming an address nothing listens
//! on, so every automatic sign-in the maintenance pass orders finds no Weles.
//! `/readyz` is read once, then one request for `best` is sent through the
//! real `brama probe`, signed as the agent the caller names, which starts a
//! maintenance pass. `/readyz` is read again each time it changes, until it
//! reports a pass newer than the one before the probe; it then has to name,
//! under `automatic_sign_in.blocked`, a member blocked by `weles_unreachable`
//! with the address it tried, and must not ask anything of an operator for it
//! (`operator_action_required` false), because the next pass signs in again
//! by itself once Weles answers.
#[path = "../subscriptions/support.rs"]
mod support;

use serde_json::{json, Value};
use support::{required, Run};

/// One readiness answer, read through the workshop's named wait. With
/// `after`, the gateway answers only once its readiness differs from the
/// answer that digest names, so no reading is asked back to back.
async fn readiness(client: &reqwest::Client, url: &str, after: Option<&str>) -> (u16, Value) {
    let request = match after {
        Some(digest) => client.get(url).query(&[("after", digest)]),
        None => client.get(url),
    };
    let answered = stado_wait::request(request).await;
    let response = answered.expect("readiness answers");
    let status = response.status().as_u16();
    (status, response.json().await.expect("readiness JSON"))
}

/// The digest a readiness answer names.
fn digest(body: &Value) -> String {
    let named = body["digest"].as_str();
    named.expect("readiness names its digest").to_owned()
}

/// Whether a blocked sign-in found no Weles.
fn by_weles(row: &Value) -> bool {
    row["blocked_by"] == "weles_unreachable"
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "Requires a real isolated gateway whose pool the ledger refuses, BRAMA_WELES_URL naming an address nothing listens on, a client bearer role, and an agent with its signing role"]
async fn a_sign_in_with_weles_down_is_named_in_readiness_and_asks_no_operator() {
    let url = required("BRAMA_READINESS_URL");
    let weles = required("BRAMA_WELES_URL");
    let bearer_role = required("CLIENT_BEARER_ROLE");
    let agent = required("AGENT");
    let signing_role = required("SIGNING_ROLE");
    let client = reqwest::Client::new();

    let mut run = Run::new();
    let (status, before) = readiness(&client, &url, None).await;
    run.report["before"] = json!({ "status": status, "body": before });
    run.save();
    let passed_before = before["maintenance"]["last_pass_unix"].as_u64();
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
    run.report["probe"] = json!({
        "status": output.status.code(),
        "stdout": String::from_utf8_lossy(&output.stdout),
        "stderr": String::from_utf8_lossy(&output.stderr),
    });
    run.save();

    // The refused probe starts one maintenance pass; each reading waits for
    // readiness to change, and the loop ends at the first pass newer than the
    // one before the probe. Readiness declares no maintenance interval to
    // bound it by, so the bound is the event itself, said by the named wait.
    let mut readings = Vec::new();
    let mut seen = digest(&before);
    let after = loop {
        let (status, body) = readiness(&client, &url, Some(&seen)).await;
        readings.push(json!({ "status": status, "body": body }));
        let newer = match (
            body["maintenance"]["last_pass_unix"].as_u64(),
            passed_before,
        ) {
            (Some(now), Some(then)) => now > then,
            (Some(_), None) => true,
            (None, _) => false,
        };
        if newer {
            break body;
        }
        seen = digest(&body);
    };
    run.report["readings"] = json!(readings);
    run.save();

    let listed = after["automatic_sign_in"]["blocked"].as_array();
    let blocked = listed.expect("readiness lists blocked sign-ins");
    let unreachable: Vec<&Value> = blocked.iter().filter(|row| by_weles(row)).collect();
    assert!(
        !unreachable.is_empty(),
        "no sign-in is named blocked by an unreachable Weles: {after}"
    );
    let tried = weles.trim_end_matches('/');
    let names_tried = |row: &&Value| {
        let detail = row["detail"].as_str();
        detail.is_some_and(|detail| detail.contains(tried))
    };
    assert!(
        unreachable.iter().all(names_tried),
        "a Weles-down block does not name the address it tried ({weles}): {after}"
    );
    let denied = after["denied"].as_array();
    let untagged = after["unroutable_accounts"].as_array();
    let only_weles = denied.is_some_and(Vec::is_empty)
        && untagged.is_some_and(Vec::is_empty)
        && blocked.iter().all(by_weles);
    assert!(
        only_weles,
        "the fixture holds a cause besides Weles being down, so it proves nothing: {after}"
    );
    assert_eq!(
        after["operator_action_required"], false,
        "Weles being down asks something of an operator, though the next pass retries: {after}"
    );
    run.report["result"] = json!("passed");
    run.save();
}
