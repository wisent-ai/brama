//! Recover a missing authenticator through the same sign-in entry point used
//! by the CLI, Desktop and automatic renewal. A saved seed is not a signed-in
//! subscription: after confirmed enrolment, resolve the changed account and
//! run authentication and persisted-credential verification again.

use serde_json::{json, Value};

use super::verdict::{verdict, FAILED};
use super::{enrol_authenticator, weles_provider, SignInError, SignInOptions};

const MISSING_SEED: &str = "google_2fa_material_missing";

pub(super) async fn execute(options: &SignInOptions) -> Result<Value, SignInError> {
    let result = super::execute(options).await?;
    if result.get("result").and_then(Value::as_str) != Some(FAILED)
        || result.pointer("/failure/code").and_then(Value::as_str) != Some(MISSING_SEED)
    {
        return Ok(result);
    }
    // The public verdict calls this field `account`; the writer consumes the
    // resolved identity's `account_ref`. Move it without losing provenance.
    let mut identity = result;
    let account = identity["account"].take();
    identity["account_ref"] = account;
    identity["http_status"] = Value::Null;
    let subscription = identity
        .get("subscription_id")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            SignInError::Dependency("the missing-seed refusal names no subscription".into())
        })?;
    let login = identity
        .get("login_item")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            SignInError::Dependency("the missing-seed refusal names no login item".into())
        })?;
    let provider = weles_provider(&options.provider).ok_or_else(|| {
        SignInError::Dependency("the subscription has no supported authenticator enrolment".into())
    })?;
    let enrolment = match enrol_authenticator(
        &options.provider,
        provider,
        subscription,
        Some(login),
    )
    .await
    {
        Ok(enrolment) => enrolment,
        Err(error) => {
            return Ok(verdict(
                options,
                &identity,
                FAILED,
                error.to_string(),
                json!({
                    "code": "authenticator_enrolment_failed", "stage": "authenticator_enrolment",
                    "browser_started": null, "retryable": false,
                    "cause": error.blocked().map(|blocked| blocked.to_json()),
                }),
                Value::Null,
            ));
        }
    };
    if !enrolment.ok() {
        return Ok(verdict(
            options,
            &identity,
            FAILED,
            enrolment.detail,
            json!({
                "code": "authenticator_enrolment_unconfirmed", "stage": "authenticator_enrolment",
                "run_id": enrolment.run_id, "seed_present": enrolment.seed_present,
                "confirmed": enrolment.confirmed,
                "browser_started": null, "retryable": false,
            }),
            Value::Null,
        ));
    }
    // Bind the continuation to the exact subscription and login just repaired,
    // even when the original caller allowed single-account resolution.
    let resumed = SignInOptions {
        provider: options.provider.clone(),
        subscription_id: Some(subscription.to_owned()),
        login_item: Some(login.to_owned()),
        reason: options.reason.clone(),
        progress: options.progress.clone(),
    };
    // Deliberately call the one-attempt operation, not this recovery wrapper:
    // another missing-seed refusal must not enrol a second authenticator.
    super::execute(&resumed).await
}
