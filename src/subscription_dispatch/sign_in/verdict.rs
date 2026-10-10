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
        "second_factor": identity.get("second_factor"),
        "run_id": identity.get("run_id"),
        "stages": identity.get("stages"),
        "operator_request": identity.get("operator_request"),
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
    // A cooldown that cannot be read stops new sign-ins and says why, rather
    // than driving a browser under a pacing nobody declared.
    let cooling_down = match crate::subscription_dispatch::refresh_sweep::sign_in_cooldown() {
        Ok(Some(cooldown)) => !crate::journal::subscription_sign_in_due(id, cooldown),
        Ok(None) => false,
        Err(refusal) => {
            tracing::warn!(subscription = %id, "{refusal}");
            true
        }
    };
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
    // wedges the subscription: one `IncompleteMessage` on POST /reauth, and
    // every later sign-in - the sweep's and the operator's - replays that
    // verdict instead of running, while the account revision it is keyed to
    // has no reason to change. Nor does the cooldown hold it: Weles admits
    // a sign-in of the same account revision into the run already under way
    // and answers it with everything that run has reported, so asking again
    // reattaches to the browser that is still waiting (a laptop's forward
    // restarting under a 5-minute Google prompt was enough to lose it)
    // instead of starting another challenge.
    // The operator ended the run (`weles runs cancel`): the provider was not
    // asked anything that could be held against the account.
    let unknown_effect = matches!(
        code,
        Some("weles_execution_unconfirmed" | "authentication_response_invalid" | "run_cancelled")
    );
    same_account
        && failed
        && !unknown_effect
        && ((rejected && (credential_rejected || same_executor)) || (cooling_down && same_executor))
}

pub(super) fn observed_failure(id: &str) -> Option<Blocked> {
    let current = crate::subscription_dispatch::usage::usage_for(id);
    if let Some(check) = current
        .as_ref()
        .and_then(|entry| entry.sign_in_check_failure.as_ref())
    {
        let active_after_check = current
            .as_ref()
            .and_then(|entry| entry.credential.as_ref())
            .is_some_and(|credential| {
                credential.state == crate::subscription_dispatch::usage::CredentialState::Active
                    && credential.recorded_at_ms >= check.at_ms
            });
        let completed_after_check = crate::journal::latest_subscription_sign_in(id)
            .and_then(|attempt| attempt.get("at_ms").and_then(Value::as_i64))
            .is_some_and(|at| at >= check.at_ms);
        if !active_after_check && !completed_after_check {
            return Some(Blocked::Operation {
                code: check.code.clone(),
                stage: "automatic_sign_in_check".into(),
                detail: check.detail.clone(),
                status: None,
            });
        }
    }
    let previous = crate::journal::latest_subscription_sign_in(id)?;
    if previous.get("result").and_then(Value::as_str) != Some(FAILED) {
        return None;
    }
    let began = previous
        .get("started_at_ms")
        .or_else(|| previous.get("at_ms"))
        .and_then(Value::as_i64);
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
