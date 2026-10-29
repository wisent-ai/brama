mod effect;
mod observed;
use super::blocked::with_holds;
use super::{weles_provider, SignInOptions};
pub use effect::effect_unknown;
pub use observed::observed_failure;
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
            // A newest record missing its result or time is a damaged line.
            match crate::journal::latest_subscription_sign_in(id) {
                Ok(Some(latest)) => match (
                    latest.get("result").and_then(Value::as_str),
                    latest.get("at").and_then(Value::as_str),
                ) {
                    (Some(result), Some(at)) => format!("{id}: last {result} at {at}"),
                    _ => format!(
                        "{id}: its newest sign-in record states no result or time: {latest}"
                    ),
                },
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
