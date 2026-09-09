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
    let failed = previous.get("result").and_then(Value::as_str) == Some(FAILED);
    let rejected = previous
        .pointer("/failure/browser_started")
        .and_then(Value::as_bool)
        != Some(false)
        && previous
            .pointer("/failure/retryable")
            .and_then(Value::as_bool)
            == Some(false);
    let credential_or_unknown_effect = matches!(
        previous.pointer("/failure/code").and_then(Value::as_str),
        Some(
            "provider_challenge_refused"
                | "GOOGLE_PASSWORD_REJECTED"
                | "weles_execution_unconfirmed"
                | "authentication_response_invalid"
                | "oauth_identity_mismatch"
        )
    );
    let cooling_down = !crate::journal::subscription_sign_in_due(
        id,
        crate::subscription_dispatch::refresh_sweep::sign_in_cooldown(),
    );
    (same_account
        && failed
        && ((rejected && (credential_or_unknown_effect || same_executor))
            || (cooling_down && same_executor)))
        .then_some(previous)
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
