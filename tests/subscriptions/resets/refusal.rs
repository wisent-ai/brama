//! Requires a real member with a retained offer and an actual reset-report refusal.
//! The provider usage endpoint must still answer so listing reaches reset reporting.
//! This journey neither changes credentials nor redeems a reset.

use super::{required, Run};
use serde_json::json;

#[test]
#[ignore = "Requires a real reset-report refusal after a previously observed saved-credit offer"]
fn failed_reset_report_retains_last_offer_and_persists_actual_cause() {
    let mut run = Run::new();
    let member = required("RESET_REFUSAL_MEMBER");
    let expected = required("EXPECTED_RESET_REFUSAL");
    assert!(
        !expected.trim().is_empty(),
        "a specific observed refusal is required"
    );
    let before = run.member(&member, false);
    let offer = before["resets"]["offer"]
        .as_object()
        .expect("the real member must already have a provider-observed reset offer");
    let observed = offer["observed_at_ms"]
        .as_i64()
        .expect("retained offer observation time");
    let previous_attempt = before["resets"]["attempted_at_ms"]
        .as_i64()
        .expect("previous real report attempt");

    let fresh = run.member(&member, true);
    let cause = fresh["resets"]["error"]
        .as_str()
        .expect("the actual reset endpoint must refuse; a healthy report is not this journey");
    assert!(
        cause.contains(&expected),
        "unexpected reset refusal: {cause}"
    );
    let attempted = fresh["resets"]["attempted_at_ms"]
        .as_i64()
        .expect("failed read attempt time");
    assert!(
        attempted > previous_attempt,
        "no new reset report attempt was observed"
    );
    assert!(attempted >= observed);
    assert_eq!(fresh["resets"]["offer"], before["resets"]["offer"]);
    assert_eq!(fresh["reset_redemption"], before["reset_redemption"]);

    let persisted = run.member(&member, false);
    assert_eq!(persisted["resets"], fresh["resets"]);
    assert_eq!(persisted["reset_redemption"], before["reset_redemption"]);
    run.report["result"] = json!("passed");
    run.save();
}
