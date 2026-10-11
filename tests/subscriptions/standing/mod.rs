//! `brama subscription list` prints where each member stands and what the
//! last purchase did, from the same pool document Brama Desktop reads.
//! Runs against a real isolated gateway holding a retired member
//! (RETIRED_MEMBER, retired with a recorded cause) and reads its text and
//! JSON output; nothing is bought, signed in or retired here.
use super::{required, Run};
use serde_json::{json, Value};

/// The text output names the retired member's cause, the newest sign-in of
/// every other member, and the last purchase of every declared provider,
/// in the words the JSON document carries.
#[test]
#[ignore = "Requires an isolated real gateway with a retired member whose retirement recorded a cause"]
fn list_prints_membership_standing_and_last_purchase() {
    let mut run = Run::new();
    let member = required("RETIRED_MEMBER");
    let listed = run.command(&["subscription", "list", "--json"]);
    assert!(listed.status.success());
    let pool: Value = serde_json::from_slice(&listed.stdout).expect("real pool document");
    let printed = run.command(&["subscription", "list"]);
    assert!(printed.status.success());
    let text = String::from_utf8_lossy(&printed.stdout);

    let rows = pool["subscriptions"].as_array().expect("pool rows");
    let retired = rows
        .iter()
        .find(|row| row["id"] == member)
        .expect("the retired fixture is in the pool");
    assert_eq!(retired["retired"], true, "fixture must be retired");
    let cause = retired["credential"]["cause"]
        .as_str()
        .expect("the retirement recorded a cause");
    assert!(
        text.contains(&format!("retired: {cause}")),
        "the retired cause is printed: {text}"
    );

    for row in rows.iter().filter(|row| row["retired"] != true) {
        match row["sign_in"].as_object() {
            Some(sign_in) => assert!(
                text.contains(&format!("last sign-in: {}", sign_in["result"])),
                "the newest sign-in of {} is printed: {text}",
                row["id"]
            ),
            None => assert!(
                text.contains("membership:"),
                "a member without a sign-in has its standing printed: {text}"
            ),
        }
    }

    let acquisition = pool["acquisition"]
        .as_object()
        .expect("the pool document carries the acquisition object");
    assert!(!acquisition.is_empty(), "providers.json declares providers");
    for (provider, latest) in acquisition {
        let line = match latest.as_object() {
            Some(record) => format!("last purchase of {provider}: {}", record["result"]),
            None => format!("last purchase of {provider}: none journaled"),
        };
        assert!(text.contains(&line), "{line} is printed: {text}");
        if let Some(record) = latest.as_object() {
            assert!(
                matches!(
                    record["result"].as_str(),
                    Some("requested") | Some("acquired") | Some("failed")
                ),
                "refusals are decisions, not journaled purchases: {}",
                record["result"]
            );
        }
    }
    run.report["result"] = json!("passed");
    run.save();
}
