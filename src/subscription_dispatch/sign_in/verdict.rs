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
        "subscription_id": options.subscription_id,
        "provider": options.provider,
        "login_item": identity.get("login_item").cloned().unwrap_or_else(|| json!(options.login_item)),
        "subscription_item": identity.get("subscription_item"),
        "account": identity.get("account_ref"),
        "source_revision": identity.get("source_revision"),
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
pub(super) fn unchanged_failed_attempt(id: &str, revision: &str) -> Option<Value> {
    let previous = crate::journal::latest_subscription_sign_in(id)?;
    let same_source = previous.get("source_revision").and_then(Value::as_str) == Some(revision);
    let failed = previous.get("result").and_then(Value::as_str) == Some(FAILED);
    let rejected = previous
        .pointer("/failure/browser_started")
        .and_then(Value::as_bool)
        == Some(true)
        && previous
            .pointer("/failure/retryable")
            .and_then(Value::as_bool)
            == Some(false);
    let cooling_down = !crate::journal::subscription_sign_in_due(
        id,
        crate::subscription_dispatch::refresh_sweep::sign_in_cooldown(),
    );
    (same_source && failed && (rejected || cooling_down)).then_some(previous)
}

pub(super) fn observed_failure(id: &str) -> Option<Blocked> {
    let previous = crate::journal::latest_subscription_sign_in(id)?;
    if previous.get("result").and_then(Value::as_str) != Some(FAILED) {
        return None;
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
