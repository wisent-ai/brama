mod effect;
pub use effect::effect_unknown;
use super::blocked::{journal_unreadable, observed_at, with_holds};
use super::{weles_provider, Blocked, SignInOptions};
use serde_json::{json, Value};

pub(super) const SIGNED_IN: &str = "signed_in";
pub(super) const FAILED: &str = "failed";
/// What the automatic sign-in did for every recorded subscription of one
/// provider, as the sentence a refusal carries beside "the provider rejected
/// every credential".
///
/// Re-authorization is this product's own job: the refresh sweep signs a
/// refused subscription back in through Weles with the login row Skarbiec
/// holds. A refusal that says only "re-authorization required" reads as an
/// instruction to a person, and asks the operator to sign in by hand for a
/// sweep that already ran and failed. The refusal says, per subscription,
/// whether that sweep does not apply, never ran, last succeeded, or failed
/// and on what.
pub fn automatic_sign_in_sentence(provider: &str) -> String {
    let recorded = crate::subscription_dispatch::usage::recorded_subscriptions();
    let mut ids: Vec<&String> = recorded
        .iter()
        .filter(|(_, usage)| usage.provider == provider)
        .map(|(id, _)| id)
        .collect();
    ids.sort();
    if ids.is_empty() {
        return format!(
            "automatic sign-in: no '{provider}' subscription is recorded in the usage ledger"
        );
    }
    if let Err(cause) = weles_provider(provider) {
        return format!("automatic sign-in: {cause}; no independent grant was obtained");
    }
    let states: Vec<String> = ids
        .iter()
        .copied()
        .map(|id| {
            match crate::journal::is_retired(id) {
                Ok(true) => return format!("{id}: retired"),
                Ok(false) => {}
                Err(cause) => {
                    return format!("{id}: whether its owner retired it could not be read: {cause}")
                }
            }
            if let Some(failure) = observed_failure(id) {
                return format!("{id}: failed ({}: {})", failure.code(), failure.detail());
            }
            match crate::journal::latest_subscription_sign_in(id) {
                Ok(Some(latest)) => format!(
                    "{id}: last {} at {}",
                    latest
                        .get("result")
                        .and_then(Value::as_str)
                        .unwrap_or("ran"),
                    latest
                        .get("at")
                        .and_then(Value::as_str)
                        .unwrap_or("an unrecorded time")
                ),
                Ok(None) => format!("{id}: never ran"),
                Err(cause) => format!("{id}: its sign-in history could not be read: {cause}"),
            }
        })
        .collect();
    format!("automatic sign-in: {}", with_holds(&ids, states).join("; "))
}

pub(super) fn verdict(
    options: &SignInOptions,
    identity: &Value,
    result: &str,
    detail: String,
    failure: Value,
    refresh: Value,
) -> Value {
    let mut output = json!({
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
    // The journal is what later stops a repeat of a final refusal; a verdict
    // it could not keep says so instead of looking recorded.
    if let Err(error) = crate::journal::record_subscription_sign_in(&output) {
        output["journal_failure"] = json!(error);
    }
    output
}

/// Repeating a failed provider challenge with unchanged source data cannot
/// repair it. A new Skarbiec identity revision permits a new, attributable run.
pub(super) fn unchanged_failed_attempt(
    id: &str,
    account_revision: &str,
    executor_revision: &str,
) -> Result<Option<Value>, String> {
    let Some(previous) = crate::journal::latest_subscription_sign_in(id)? else {
        return Ok(None);
    };
    let same_account =
        previous.get("account_revision").and_then(Value::as_str) == Some(account_revision);
    let same_executor =
        previous.get("source_revision").and_then(Value::as_str) == Some(executor_revision);
    // A cooldown that cannot be read stops new sign-ins and says why, rather
    // than driving a browser under a pacing nobody declared.
    let cooling_down = match crate::subscription_dispatch::refresh_sweep::sign_in_cooldown() {
        Ok(Some(cooldown)) => !crate::journal::subscription_sign_in_due(id, cooldown)?,
        Ok(None) => false,
        Err(refusal) => {
            tracing::warn!(subscription = %id, "{refusal}");
            true
        }
    };
    Ok(
        stops_a_new_attempt(&previous, same_account, same_executor, cooling_down)
            .then_some(previous),
    )
}

/// The recorded verdict handed back in place of a run, saying so: `replayed`
/// is set, `replay_of_at` names the instant the recorded attempt completed,
/// and `detail` opens with why no browser was driven, so a reader of the
/// lines or the JSON never takes an old failure for this call's result.
/// An incomplete journal record refuses replay without starting a new run.
pub(super) fn replayed(
    mut previous: Value,
    account_revision: &str,
    subscription_id: &str,
) -> Result<Value, super::SignInError> {
    let required = |field: &str| {
        previous
            .get(field)
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .map(str::to_owned)
            .ok_or_else(|| super::SignInError::Dependency(format!(
                "cannot replay sign-in for subscription {subscription_id}: recorded {field} is missing, empty or not a string; no browser was started"
            )))
    };
    let recorded_at = required("at")?;
    let detail = required("detail")?;
    previous["replayed"] = json!(true);
    previous["replay_of_at"] = json!(recorded_at);
    previous["detail"] = json!(format!(
        "not run: the failed attempt recorded at {recorded_at} stands, because the account \
         (revision {account_revision}) and the sign-in executor are unchanged since it and \
         the provider's refusal was final; change the login row or wait out the sign-in \
         cooldown to run again. That attempt: {detail}"
    ));
    Ok(previous)
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
    // Nobody knows what the provider did ([`effect_unknown`]). That is no
    // evidence a repeat cannot repair the account, and treating it as such
    // wedges the subscription: every later sign-in replays that verdict while
    // the account revision it is keyed to has no reason to change. Nor does
    // the cooldown hold it: Weles admits a sign-in of the same account
    // revision into the run already under way, so asking again reattaches to
    // the browser that is still waiting instead of starting another challenge.
    let unknown_effect = code.is_some_and(effect_unknown);
    same_account
        && failed
        && !unknown_effect
        && ((rejected && (credential_rejected || same_executor)) || (cooling_down && same_executor))
}

pub fn observed_failure(id: &str) -> Option<Blocked> {
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
        let completed_after_check = match crate::journal::latest_subscription_sign_in(id) {
            Ok(latest) => latest
                .and_then(|attempt| attempt.get("at_ms").and_then(Value::as_i64))
                .is_some_and(|at| at >= check.at_ms),
            Err(cause) => return Some(journal_unreadable(cause)),
        };
        if !active_after_check && !completed_after_check {
            return Some(Blocked::Operation {
                code: check.code.clone(),
                stage: "automatic_sign_in_check".into(),
                detail: format!(
                    "{} ({})",
                    check.detail,
                    observed_at(Some(check.at_ms), "checked")
                ),
                status: None,
            });
        }
    }
    let previous = match crate::journal::latest_subscription_sign_in(id) {
        Ok(previous) => previous?,
        // Whether the last sign-in failed lives in the journal; unreadable,
        // the member is held with the journal's cause rather than called clear.
        Err(cause) => return Some(journal_unreadable(cause)),
    };
    if previous.get("result").and_then(Value::as_str) != Some(FAILED) {
        return None;
    }
    // An attempt refused before any run (identity, login lookup) records
    // `started_at_ms` as null, not absent: read it as a number, so the
    // completion instant stands in for a run start that never happened.
    let began = previous
        .get("started_at_ms")
        .and_then(Value::as_i64)
        .or_else(|| previous.get("at_ms").and_then(Value::as_i64));
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
    let recorded = |pointer: &str| previous.pointer(pointer).and_then(Value::as_str);
    let status = match previous.pointer("/failure/http_status").and_then(Value::as_u64) {
        Some(value) => match u16::try_from(value) {
            Ok(status) => Some(status),
            Err(error) => {
                return Some(journal_unreadable(format!(
                    "the last recorded sign-in of {id} names HTTP status {value}: {error}"
                )))
            }
        },
        None => None,
    };
    // A record older than failure codes reads in the documented words for it.
    Some(Blocked::Operation {
        code: match recorded("/failure/code") {
            Some(code) => code.into(),
            None => "authentication_outcome_unclassified".into(),
        },
        stage: match recorded("/failure/stage") {
            Some(stage) => stage.into(),
            None => "stage not recorded".into(),
        },
        detail: format!(
            "{} ({})",
            match recorded("/detail") {
                Some(detail) => detail,
                None => "The previous authentication attempt did not retain its outcome",
            },
            observed_at(began, "attempted")
        ),
        status,
    })
}
