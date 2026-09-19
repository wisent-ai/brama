use super::{Blocked, SignInOptions};
use serde_json::{json, Value};

pub(super) const SIGNED_IN: &str = "signed_in";
pub(super) const FAILED: &str = "failed";

pub(super) fn verdict(
    options: &SignInOptions,
    identity: &Value,
    result: &str,
    detail: String,
    failure: Value,
    refresh: Value,
) -> Value {
    let output = json!({
        "subscription_id": identity.get("subscription_id").cloned().unwrap_or_else(|| json!(options.subscription_id)),
        "provider": options.provider,
        "login_item": identity.get("login_item").cloned().unwrap_or_else(|| json!(options.login_item)),
        "subscription_item": identity.get("subscription_item"),
        "account": identity.get("account_ref"),
        "source_revision": identity.get("source_revision"),
        "account_revision": identity.get("account_revision"),
        "started_at_ms": identity.get("started_at_ms"),
        "http_status": identity.get("http_status"),
        "reason": options.reason,
        "result": result,
        "detail": detail,
        "failure": failure,
        "refresh": refresh,
    });
    crate::journal::record_subscription_sign_in(&output);
    output
}

/// Repeating a failed provider challenge with unchanged source data cannot
/// repair it. A new Skarbiec identity revision permits a new, attributable run.
pub(super) fn unchanged_failed_attempt(
    id: &str,
    account_revision: &str,
    executor_revision: &str,
) -> Option<Value> {
    let previous = crate::journal::latest_subscription_sign_in(id)?;
    let same_account =
        previous.get("account_revision").and_then(Value::as_str) == Some(account_revision);
    let same_executor =
        previous.get("source_revision").and_then(Value::as_str) == Some(executor_revision);
    let cooling_down = !crate::journal::subscription_sign_in_due(
        id,
        crate::subscription_dispatch::refresh_sweep::sign_in_cooldown(),
    );
    stops_a_new_attempt(&previous, same_account, same_executor, cooling_down).then_some(previous)
}

/// Whether a recorded attempt stands in the way of running another one.
///
/// Split out of the lookup above so the rule is readable and testable without
/// a journal on disk: everything it reads is either the recorded verdict or a
/// fact the caller established.
fn stops_a_new_attempt(
    previous: &Value,
    same_account: bool,
    same_executor: bool,
    cooling_down: bool,
) -> bool {
    let failed = previous.get("result").and_then(Value::as_str) == Some(FAILED);
    let rejected = previous
        .pointer("/failure/browser_started")
        .and_then(Value::as_bool)
        != Some(false)
        && previous
            .pointer("/failure/retryable")
            .and_then(Value::as_bool)
            == Some(false);
    let code = previous.pointer("/failure/code").and_then(Value::as_str);
    // The provider saw the credential and said no. Handing it the same
    // credential again spends a challenge to be told the same thing.
    let credential_rejected = matches!(
        code,
        Some("provider_challenge_refused" | "GOOGLE_PASSWORD_REJECTED" | "oauth_identity_mismatch")
    );
    // Nobody knows what the provider did: the HTTP exchange with Weles died
    // before an answer, or the answer was unreadable. That is not evidence
    // that a repeat cannot repair the account, and treating it as evidence
    // wedged `brama-sub-wisent-app-codex-primary` from 2026-09-18: one
    // `IncompleteMessage` on POST /reauth, and every later sign-in - the
    // sweep's and the operator's - replayed that verdict instead of running,
    // while the account revision it is keyed to had no reason to change. An
    // unconfirmed result waits for the cooldown like any other retry.
    let unknown_effect = matches!(
        code,
        Some("weles_execution_unconfirmed" | "authentication_response_invalid")
    );
    same_account
        && failed
        && ((rejected && !unknown_effect && (credential_rejected || same_executor))
            || (cooling_down && same_executor))
}

pub(super) fn observed_failure(id: &str) -> Option<Blocked> {
    let previous = crate::journal::latest_subscription_sign_in(id)?;
    if previous.get("result").and_then(Value::as_str) != Some(FAILED) {
        return None;
    }
    let began = previous
        .get("started_at_ms")
        .or_else(|| previous.get("at_ms"))
        .and_then(Value::as_i64);
    let current = crate::subscription_dispatch::usage::usage_for(id);
    if let (Some(began), Some(credential)) = (
        began,
        current.as_ref().and_then(|usage| usage.credential.as_ref()),
    ) {
        if credential.state == crate::subscription_dispatch::usage::CredentialState::Active
            && credential.recorded_at_ms >= began
        {
            return None;
        }
    }
    Some(Blocked::Operation {
        code: previous
            .pointer("/failure/code")
            .and_then(Value::as_str)
            .unwrap_or("authentication_outcome_unclassified")
            .into(),
        stage: previous
            .pointer("/failure/stage")
            .and_then(Value::as_str)
            .unwrap_or("unrecorded")
            .into(),
        detail: previous
            .get("detail")
            .and_then(Value::as_str)
            .unwrap_or("The previous authentication attempt did not retain its outcome")
            .into(),
        status: previous
            .pointer("/failure/http_status")
            .and_then(Value::as_u64)
            .and_then(|value| u16::try_from(value).ok()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn attempt(code: &str) -> Value {
        json!({
            "result": FAILED,
            "failure": {"code": code, "retryable": false, "browser_started": null},
        })
    }

    /// The state `brama-sub-wisent-app-codex-primary` was wedged in from
    /// 2026-09-18: one HTTP exchange with Weles died mid-answer, and every
    /// later sign-in replayed that verdict instead of running.
    #[test]
    fn an_unconfirmed_result_runs_again_once_the_cooldown_has_passed() {
        assert!(!stops_a_new_attempt(
            &attempt("weles_execution_unconfirmed"),
            true,
            true,
            false
        ));
        assert!(!stops_a_new_attempt(
            &attempt("authentication_response_invalid"),
            true,
            true,
            false
        ));
    }

    /// It is still a repeat, so it still waits its turn.
    #[test]
    fn an_unconfirmed_result_waits_while_it_is_cooling_down() {
        assert!(stops_a_new_attempt(
            &attempt("weles_execution_unconfirmed"),
            true,
            true,
            true
        ));
    }

    /// A credential the provider looked at and refused is not repaired by
    /// sending it again, cooldown or no cooldown.
    #[test]
    fn a_rejected_credential_still_stops_a_repeat() {
        for code in [
            "provider_challenge_refused",
            "GOOGLE_PASSWORD_REJECTED",
            "oauth_identity_mismatch",
        ] {
            assert!(
                stops_a_new_attempt(&attempt(code), true, false, false),
                "{code}"
            );
        }
    }

    /// A new Skarbiec identity revision is a new attempt, whatever failed
    /// before.
    #[test]
    fn a_changed_account_revision_always_runs() {
        assert!(!stops_a_new_attempt(
            &attempt("provider_challenge_refused"),
            false,
            true,
            true
        ));
    }
}
