//! Brama requests authentication for a Skarbiec subscription; Weles executes it.
mod worker;
pub mod blocked;
mod trajectory;
mod verdict;

use crate::subscription_dispatch::pool;
pub use blocked::{Blocked, SignInError};
use serde_json::{json, Value};
use std::time::Duration;
use verdict::{verdict, FAILED, SIGNED_IN};
use worker::api::{transport_timeout_seconds, worker_api_base, worker_api_token};

pub struct SignInOptions {
    pub provider: String,
    /// Optional exact Skarbiec login reference for an explicitly selected run.
    pub login_item: Option<String>,
    pub subscription_id: Option<String>,
    pub reason: String,
    pub login_timeout_ms: u64,
}

pub fn weles_provider(provider: &str) -> Option<&'static str> {
    match provider.trim() {
        "claude-code" => Some("claude"),
        "codex" => Some("codex"),
        "kimi" => Some("kimi"),
        _ => None,
    }
}

pub fn observed_failure(subscription_id: &str) -> Option<Blocked> {
    verdict::observed_failure(subscription_id)
}

pub async fn sign_in_provider(options: SignInOptions) -> Result<Value, SignInError> {
    let result = execute(&options).await;
    if let Err(error) = &result {
        // Only an exchange Weles actually answered is journaled. A refusal
        // taken before the worker was reached -- an unknown provider, a
        // missing startup credential, a worker that never answered, a vault
        // that names no subscription -- records nothing, because a verdict
        // here would put an attempt in the journal that was never made, and
        // an operator reading the journal during an incident would find a
        // sign-in that "failed" against a provider nothing was asked of.
        let Some(Blocked::Operation {
            code,
            stage,
            status: Some(status),
            ..
        }) = error.blocked()
        else {
            return result;
        };
        verdict(
            &options,
            &Value::Null,
            FAILED,
            error.to_string(),
            json!({
                "code": code, "stage": stage, "http_status": status,
                "browser_started": false, "retryable": true,
            }),
            Value::Null,
        );
    }
    result
}

async fn execute(options: &SignInOptions) -> Result<Value, SignInError> {
    let provider = weles_provider(&options.provider).ok_or_else(|| {
        // Naming what Weles does sign in is the whole value of this refusal:
        // an operator who typed the wrong provider needs the three that work,
        // not a restatement of the word they typed.
        SignInError::Dependency(format!(
            "Weles signs in claude-code, codex and kimi; `{}` is not one of them",
            options.provider
        ))
    })?;
    if options.reason.trim().is_empty() {
        return Err("--reason must say why this sign-in is being run"
            .to_string()
            .into());
    }
    // The host's own prerequisites are read before Skarbiec is asked
    // anything. A missing startup credential or an unreachable worker is a
    // fact about this host, and reporting it as "Skarbiec lists 0 active
    // subscriptions" sends an operator to the wrong system.
    let base = worker_api_base()
        .await
        .map_err(|detail| Blocked::WelesUnreachable { detail })?;
    let token = worker_api_token().map_err(|detail| Blocked::Operation {
        code: "weles_authentication_credential_unavailable".into(),
        stage: "admission".into(),
        detail,
        status: None,
    })?;
    let id = match options.subscription_id.as_deref() {
        Some(id) if !id.is_empty() => id.to_owned(),
        _ => {
            let entries = crate::gateway::broker::list_all_subscriptions().await?;
            let matching: Vec<_> = entries
                .iter()
                .filter(|entry| {
                    entry.provider == options.provider
                        && entry.status == "active"
                        && !crate::journal::is_retired(&entry.id)
                })
                .collect();
            if matching.len() != 1 {
                return Err(SignInError::Dependency(format!(
                    "Skarbiec lists {} active subscriptions for {}; an exact subscription id is required",
                    matching.len(), options.provider)));
            }
            matching[0].id.clone()
        }
    };
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(transport_timeout_seconds()))
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
    if let Some(previous) = verdict::unchanged_failed_attempt(
        &id,
        &resolved.account_revision,
        &resolved.source_revision,
    ) {
        return Ok(previous);
    }
    let mut identity = serde_json::to_value(&resolved).expect("resolved account serializes");
    identity["started_at_ms"] = json!(chrono::Utc::now().timestamp_millis());
    let response = match client
        .post(format!("{base}/reauth"))
        .bearer_auth(&token)
        .json(&json!({
            "provider": provider, "subscription_id": id, "login_item": resolved.login_item,
            "account_revision": resolved.account_revision, "timeout_ms": options.login_timeout_ms,
        }))
        .send()
        .await
    {
        Ok(response) => response,
        Err(error) => {
            return Ok(verdict(
                options,
                &identity,
                FAILED,
                format!("The result of POST {base}/reauth is unconfirmed: {error:?}"),
                json!({"code": "weles_execution_unconfirmed", "stage": "weles_response",
                    "browser_started": null, "retryable": false}),
                Value::Null,
            ));
        }
    };
    let status = response.status().as_u16();
    identity["http_status"] = json!(status);
    let answer: Value = match response.json().await {
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
    };
    if let Some(detail) = trajectory::refusal(&answer, status, &resolved.login_item) {
        let mut failure = answer.get("failure").filter(|value| value.is_object()).cloned().unwrap_or_else(|| json!({
            "code": answer.get("error").and_then(Value::as_str).unwrap_or("authentication_failed"),
            "stage": answer.get("stage").and_then(Value::as_str).unwrap_or("weles_execution"),
            "browser_started": null, "retryable": false,
        }));
        failure["run_id"] = answer.get("run_id").cloned().unwrap_or(Value::Null);
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
                "http_status": status, "run_id": answer.get("run_id"),
                "browser_started": true, "retryable": false}),
                Value::Null,
            ))
        }
    };
    let refreshed = refresh.get("result").and_then(Value::as_str) == Some("refreshed");
    let detail = if refreshed {
        format!("Weles authenticated Skarbiec subscription {} and Brama refreshed its persisted credential", resolved.subscription_item)
    } else {
        format!("Weles finished authentication, but Brama could not refresh the persisted credential: {}",
            refresh.get("detail").and_then(Value::as_str).unwrap_or("refresh returned no reason"))
    };
    let failure = if refreshed {
        Value::Null
    } else {
        json!({
            "code": "persisted_credential_refresh_failed", "stage": "refresh_verification",
            "http_status": status, "run_id": answer.get("run_id"),
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
