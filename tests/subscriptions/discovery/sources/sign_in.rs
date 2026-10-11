//! Real gateway, Weles and provider authentication of one selected account.
use super::{required, Run};
use serde_json::{json, Value};

#[test]
#[ignore = "Requires the selected account's independent Weles login on a dedicated host"]
fn remote_weles_sign_in_proves_the_selected_member() {
    prove_selected_member(false);
}

#[test]
#[ignore = "Requires a real Claude account whose independent Weles sign-in presents a captcha"]
fn claude_captcha_sign_in_finishes_with_a_live_grant() {
    prove_selected_member(true);
}

fn prove_selected_member(require_captcha: bool) {
    let mut run = Run::new();
    let member = required("MEMBER");
    let account = required("EXPECTED_ACCOUNT");
    let before = run.member(&member, false);
    let provider = before["provider"].as_str().expect("member provider");
    if require_captcha {
        assert_eq!(
            provider, "claude-code",
            "captcha qualification requires a Claude member"
        );
    }
    assert!(before["account"]
        .as_str()
        .expect("selected account")
        .eq_ignore_ascii_case(&account));
    let missing = run.command(&[
        "subscription",
        "sign-in",
        provider,
        "--by",
        "weles",
        "--reason",
        "real gateway sign-in test: require an exact member",
    ]);
    assert!(!missing.status.success());
    assert!(String::from_utf8_lossy(&missing.stderr).contains("requires --subscription-id"));
    let mismatch = run.command(&[
        "subscription",
        "sign-in",
        "nonexistent-provider",
        "--by",
        "weles",
        "--subscription-id",
        &member,
        "--reason",
        "real gateway sign-in test: refuse the wrong provider",
    ]);
    assert!(!mismatch.status.success());
    assert!(String::from_utf8_lossy(&mismatch.stderr).contains("no sign-in was started"));
    let unchanged = run.member(&member, false);
    assert_eq!(unchanged["sign_in"], before["sign_in"]);
    let result = run.command(&[
        "subscription",
        "sign-in",
        provider,
        "--by",
        "weles",
        "--subscription-id",
        &member,
        "--reason",
        "real gateway sign-in test: obtain this account's independent grant",
    ]);
    assert!(
        result.status.success(),
        "{} {}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    let verdict: Value = serde_json::from_slice(&result.stdout).expect("sign-in verdict JSON");
    assert_eq!(verdict["result"], "signed_in");
    assert_eq!(verdict["provider"], provider);
    assert_eq!(verdict["subscription_id"], member);
    if require_captcha {
        assert!(
            verdict["stages"]
                .as_array()
                .expect("observed Weles stages")
                .iter()
                .any(|stage| stage["stage"] == "claude_captcha_answer"),
            "the provider did not present the captcha path; this run does not qualify it"
        );
    }
    let run_id = verdict["run_id"]
        .as_str()
        .expect("Weles authentication run");
    let after = run.member(&member, false);
    assert_eq!(after["credential"]["state"], "active");
    assert_eq!(after["sign_in"]["result"], "signed_in");
    assert_eq!(after["sign_in"]["run_id"], run_id);
    assert!(after["account"]
        .as_str()
        .expect("persisted account")
        .eq_ignore_ascii_case(&account));
    run.report["result"] = json!("passed");
    run.save();
}

/// A member whose latest recorded sign-in is a final failure with the same
/// account and executor is not driven again; the recorded verdict comes back
/// and says it was not run, with the instant it stands for.
#[test]
#[ignore = "Requires a gateway whose FAILED_MEMBER holds a final failed sign-in verdict"]
fn remote_weles_sign_in_replays_a_final_failure_and_says_so() {
    let mut run = Run::new();
    let member = required("FAILED_MEMBER");
    let before = run.member(&member, false);
    let provider = before["provider"].as_str().expect("member provider");
    assert_eq!(before["sign_in"]["result"], "failed");
    assert_eq!(before["sign_in"]["failure"]["retryable"], false);
    let recorded_at = before["sign_in"]["at"]
        .as_str()
        .expect("the recorded verdict names its instant")
        .to_owned();
    let result = run.command(&[
        "subscription",
        "sign-in",
        provider,
        "--by",
        "weles",
        "--subscription-id",
        &member,
        "--reason",
        "real gateway sign-in test: a final failure is replayed, not rerun",
    ]);
    assert!(
        !result.status.success(),
        "a replayed final sign-in failure must retain a failing command status"
    );
    let verdict: Value = serde_json::from_slice(&result.stdout).expect("sign-in verdict JSON");
    assert_eq!(verdict["replayed"], true);
    assert_eq!(verdict["replay_of_at"], recorded_at);
    assert_eq!(verdict["at"], recorded_at);
    assert_eq!(verdict["result"], "failed");
    assert_eq!(verdict["provider"], provider);
    assert_eq!(verdict["subscription_id"], member);
    assert_eq!(verdict["failure"], before["sign_in"]["failure"]);
    let after = run.member(&member, false);
    assert_eq!(
        after["sign_in"], before["sign_in"],
        "no new attempt was recorded"
    );
    run.report["result"] = json!("passed");
    run.save();
}

#[test]
#[ignore = "Requires a gateway whose INVALID_REPLAY_MEMBER has an incomplete retained final failure"]
fn remote_weles_sign_in_refuses_an_incomplete_record_without_rerunning() {
    let mut run = Run::new();
    let member = required("INVALID_REPLAY_MEMBER");
    let before = run.member(&member, false);
    let provider = before["provider"].as_str().expect("member provider");
    let recorded = &before["sign_in"];
    assert_eq!(recorded["result"], "failed");
    assert_eq!(recorded["failure"]["retryable"], false);
    let invalid_field = if recorded["at"]
        .as_str()
        .is_none_or(|value| value.trim().is_empty())
    {
        "at"
    } else {
        assert!(
            recorded["detail"]
                .as_str()
                .is_none_or(|value| value.trim().is_empty()),
            "the selected retained record has no missing replay evidence"
        );
        "detail"
    };
    let result = run.command(&[
        "subscription",
        "sign-in",
        provider,
        "--by",
        "weles",
        "--subscription-id",
        &member,
        "--reason",
        "real gateway sign-in test: incomplete journal evidence must refuse replay",
    ]);
    assert!(
        !result.status.success(),
        "invalid replay evidence was accepted"
    );
    let refusal = format!(
        "{}{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(refusal.contains(&format!(
        "cannot replay sign-in for subscription {member}: recorded {invalid_field} is missing, empty or not a string"
    )));
    let after = run.member(&member, false);
    assert_eq!(
        after["sign_in"], *recorded,
        "a new attempt replaced the invalid record"
    );
    run.report["result"] = json!("passed");
    run.save();
}
