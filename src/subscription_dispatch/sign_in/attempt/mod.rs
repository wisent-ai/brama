//! One sign-in attempt: resolve the account, ask Weles to authenticate it, read
//! what Weles answered, and refresh the persisted credential it produced.

use serde_json::{json, Value};

use super::blocked::Blocked;
use super::verdict::{self, verdict, FAILED, SIGNED_IN};
use super::worker::api::{worker_api_base, worker_api_token};
use super::{recovery, trajectory, weles_provider, worker, SignInError, SignInOptions};
use crate::subscription_dispatch::pool;

pub(super) async fn execute(options: &SignInOptions) -> Result<Value, SignInError> {
    let provider = weles_provider(&options.provider).map_err(SignInError::Dependency)?;
    if options.reason.trim().is_empty() {
        return Err("--reason must say why this sign-in is being run"
            .to_string()
            .into());
    }
    // Resolve the worker first: a transport refusal must name the failed
    // service, not be reported as an empty subscription pool.
    let endpoint = worker_api_base()
        .await
        .map_err(|detail| Blocked::WelesUnreachable { detail })?;
    let base = endpoint.url.clone();
    let token = worker_api_token().map_err(|detail| Blocked::Operation {
        code: "weles_authentication_credential_unavailable".into(),
        stage: "admission".into(),
        detail,
        status: None,
    })?;
    let id = match options.subscription_id.as_deref() {
        Some(id) if !id.is_empty() => {
            // An exact id the pool does not list may be a recoverable account
            // (complete identity, lost routing mark): admitted here, so its
            // sign-in banks the independent grant that makes it routable
            // instead of Weles answering "subscription not found".
            let listed = crate::gateway::broker::list_all_subscriptions().await?;
            if !listed.iter().any(|entry| entry.id == id) {
                recovery::membership::admit_recoverable(id)
                    .await
                    .map_err(SignInError::Dependency)?
                    .ok_or_else(|| {
                        SignInError::Dependency(format!(
                            "Skarbiec lists no subscription {id} for {}: neither in the pool nor as a recoverable account",
                            options.provider
                        ))
                    })?;
            }
            id.to_owned()
        }
        _ => {
            let entries = crate::gateway::broker::list_all_subscriptions().await?;
            let mut matching = Vec::new();
            for entry in &entries {
                if entry.provider == options.provider
                    && entry.status == "active"
                    && !crate::journal::is_retired(&entry.id).map_err(SignInError::Dependency)?
                {
                    matching.push(entry);
                }
            }
            let [only] = matching.as_slice() else {
                return Err(SignInError::Dependency(format!(
                    "Skarbiec lists {} active subscriptions for {}; an exact subscription id is required",
                    matching.len(), options.provider)));
            };
            only.id.clone()
        }
    };
    let client = reqwest::Client::builder()
        .build()
        .map_err(|error| SignInError::Dependency(format!("Weles HTTP client: {error}")))?;
    let resolved = worker::account::resolve(
        &client,
        &base,
        &token,
        provider,
        &id,
        options.login_item.as_deref(),
    )
    .await?;
    // The safety stop is bound to the actual account data read from Skarbiec,
    // not merely to the existence of any past attempt or to a missing tag.
    // The verdict handed back is the recorded one, said so: a caller that read
    // a two-day-old failure as this run's answer went looking for a browser
    // that never opened.
    if let Some(previous) = verdict::unchanged_failed_attempt(
        &id,
        &resolved.account_revision,
        &resolved.source_revision,
    )
    .map_err(SignInError::Dependency)?
    {
        return verdict::replayed(previous, &resolved.account_revision, &id);
    }
    let mut identity = serde_json::to_value(&resolved).map_err(|error| {
        SignInError::Dependency(format!("the resolved account cannot be recorded: {error}"))
    })?;
    identity["started_at_ms"] = json!(chrono::Utc::now().timestamp_millis());
    let request = client
        .post(format!("{base}/reauth"))
        .bearer_auth(&token)
        .json(&json!({
            "provider": provider, "subscription_id": id, "login_item": resolved.login_item,
            "account_revision": resolved.account_revision,
        }));
    let response = match stado_wait::request(request).await {
        Ok(response) => response,
        // A connection Weles never accepted started nothing: that is Weles
        // unreachable, named as such, not a run whose result is unknown.
        Err(error) if error.is_connect() => {
            return Ok(verdict(
                options,
                &identity,
                FAILED,
                format!(
                    "Weles did not accept the connection for POST {base}/reauth, so no sign-in was started: {error:?}. {}",
                    endpoint.whereabouts()
                ),
                json!({"code": "weles_unreachable", "stage": "weles_request",
                    "browser_started": false, "retryable": true}),
                Value::Null,
            ));
        }
        Err(error) => {
            return Ok(verdict(
                options,
                &identity,
                FAILED,
                format!(
                    "The result of POST {base}/reauth is unconfirmed: {error}. {}",
                    endpoint.whereabouts()
                ),
                json!({"code": "weles_execution_unconfirmed", "stage": "weles_response",
                    "browser_started": null, "retryable": true}),
                Value::Null,
            ));
        }
    };
    let status = response.status().as_u16();
    identity["http_status"] = json!(status);
    // An answer is streamed progress only when its content type says so; a
    // header that is not visible ASCII cannot name that type.
    let streamed = match response.headers().get(reqwest::header::CONTENT_TYPE) {
        Some(value) => value
            .to_str()
            .is_ok_and(|value| value.starts_with(worker::progress::PROGRESS_CONTENT_TYPE)),
        None => false,
    };
    let mut answer: Value = if streamed {
        match worker::progress::read(response, options.progress.as_ref()).await {
            Ok(observed) => {
                observed.record(&mut identity);
                match observed.result {
                    Some(result) => result,
                    None => {
                        return Ok(verdict(
                            options,
                            &identity,
                            FAILED,
                            format!(
                                "Weles ended the answer without a result. {}",
                                endpoint.whereabouts()
                            ),
                            json!({"code": "weles_execution_unconfirmed", "stage": "weles_response",
                                "browser_started": null, "retryable": true}),
                            Value::Null,
                        ));
                    }
                }
            }
            Err(stopped) => {
                let (observed, detail) = *stopped;
                observed.record(&mut identity);
                let stage = match observed.last_stage() {
                    Some(stage) => stage,
                    None => "weles_response",
                };
                return Ok(verdict(
                    options,
                    &identity,
                    FAILED,
                    format!(
                        "{detail}; {}. {}",
                        observed.whereabouts(),
                        endpoint.whereabouts()
                    ),
                    json!({"code": "weles_execution_unconfirmed", "stage": stage,
                        "run_id": observed.run_id, "browser_started": null, "retryable": true}),
                    Value::Null,
                ));
            }
        }
    } else {
        match response.json().await {
            Ok(answer) => answer,
            Err(error) => {
                return Ok(verdict(
                    options,
                    &identity,
                    FAILED,
                    format!(
                        "Weles HTTP {status} returned an unreadable authentication result: {error}"
                    ),
                    json!({"code": "authentication_response_invalid", "stage": "weles_response",
                    "http_status": status, "browser_started": null, "retryable": false}),
                    Value::Null,
                ))
            }
        }
    };
    let refusal = trajectory::refusal(&answer, status, &resolved.login_item);
    // What Weles did not report is recorded as absent, not as a value.
    identity["second_factor"] = taken(&mut answer, "second_factor");
    identity["run_id"] = taken(&mut answer, "run_id");
    if let Some(detail) = refusal {
        let mut failure = refusal_failure(&answer);
        failure["run_id"] = identity["run_id"].clone();
        failure["weles_http_status"] = json!(status);
        return Ok(verdict(
            options,
            &identity,
            FAILED,
            detail,
            failure,
            Value::Null,
        ));
    }
    let refresh = match pool::refresh_subscription(&options.provider, &id, &options.reason).await {
        Ok(refresh) => refresh,
        Err(detail) => {
            return Ok(verdict(
                options,
                &identity,
                FAILED,
                detail,
                json!({"code": "persisted_credential_refresh_failed", "stage": "refresh_verification",
                "http_status": status, "run_id": identity.get("run_id"),
                "browser_started": true, "retryable": false}),
                Value::Null,
            ))
        }
    };
    let refreshed = refresh.get("result").and_then(Value::as_str) == Some("refreshed");
    let detail = if refreshed {
        format!("Weles authenticated Skarbiec subscription {} and Brama refreshed its persisted credential", resolved.subscription_item)
    } else {
        let reason = match refresh.get("detail").and_then(Value::as_str) {
            Some(reason) => reason.to_string(),
            None => format!("the refresh answered without a reason: {refresh}"),
        };
        format!("Weles finished authentication, but Brama could not refresh the persisted credential: {reason}")
    };
    let failure = if refreshed {
        Value::Null
    } else {
        json!({
            "code": "persisted_credential_refresh_failed", "stage": "refresh_verification",
            "http_status": status, "run_id": identity.get("run_id"),
            "browser_started": true, "retryable": false,
        })
    };
    Ok(verdict(
        options,
        &identity,
        if refreshed { SIGNED_IN } else { FAILED },
        detail,
        failure,
        refresh,
    ))
}

/// A field Weles reported, moved out of its answer; JSON null when it did not
/// report one.
fn taken(answer: &mut Value, field: &str) -> Value {
    match answer.get_mut(field) {
        Some(value) => value.take(),
        None => Value::Null,
    }
}

/// The failure a refusal is recorded with: the failure object `{code, stage}`
/// Weles reports with every refusal. An answer that refuses without one is
/// Weles's response being incomplete, recorded as `weles_response_incomplete`
/// at Brama's own stage of reading that response, naming what was missing and
/// carrying the error and stage Weles did state word for word (null where it
/// stated none). Brama supplies no code or stage in Weles's name.
fn refusal_failure(answer: &Value) -> Value {
    let missing = match answer.get("failure") {
        Some(failure) if failure.is_object() => return failure.clone(),
        Some(failure) => format!("Weles refused with a failure that is not an object: {failure}"),
        None => "Weles refused without the failure object {code, stage} its reauth answer carries"
            .to_string(),
    };
    json!({
        "code": "weles_response_incomplete", "stage": "weles_response", "detail": missing,
        "weles_error": stated(answer, "error"), "weles_stage": stated(answer, "stage"),
        "browser_started": null, "retryable": false,
    })
}

/// A field Weles stated, copied as it stated it; JSON null when it stated none.
fn stated(answer: &Value, field: &str) -> Value {
    match answer.get(field) {
        Some(value) => value.clone(),
        None => Value::Null,
    }
}
