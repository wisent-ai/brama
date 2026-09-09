//! Brama requests authentication for a Skarbiec subscription; Weles executes it.
mod account;
pub mod blocked;
mod trajectory;
mod verdict;
mod worker_api;

use crate::subscription_dispatch::pool;
pub use blocked::{Blocked, SignInError};
use serde_json::{json, Value};
use std::time::Duration;
use verdict::{verdict, FAILED, SIGNED_IN};
use worker_api::{transport_timeout_seconds, worker_api_base, worker_api_token};

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
        let (code, stage, status) = match error.blocked() {
            Some(Blocked::Operation {
                code,
                stage,
                status,
                ..
            }) => (code.as_str(), stage.as_str(), *status),
            Some(blocked) => (blocked.code(), "identity", None),
            None => ("authentication_preflight_failed", "preflight", None),
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
        SignInError::Dependency(format!(
            "Weles does not support subscription authentication for {}",
            options.provider
        ))
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
    if options.reason.trim().is_empty() {
        return Err("--reason must say why this sign-in is being run"
            .to_string()
            .into());
    }
    let base = worker_api_base()
        .await
        .map_err(|detail| Blocked::WelesUnreachable { detail })?;
    let token = worker_api_token().map_err(|detail| Blocked::Operation {
        code: "weles_authentication_credential_unavailable".into(),
        stage: "admission".into(),
        detail,
        status: None,
    })?;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(transport_timeout_seconds()))
        .build()
        .map_err(|error| SignInError::Dependency(format!("Weles HTTP client: {error}")))?;
    let resolved = account::resolve(
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
    if let Some(previous) = verdict::unchanged_failed_attempt(&id, &resolved.source_revision) {
        return Ok(previous);
    }
    let identity = serde_json::to_value(&resolved).expect("resolved account serializes");
    let response = client
        .post(format!("{base}/reauth"))
        .bearer_auth(&token)
        .json(&json!({
            "provider": provider, "subscription_id": id, "login_item": resolved.login_item,
            "source_revision": resolved.source_revision, "timeout_ms": options.login_timeout_ms,
        }))
        .send()
        .await
        .map_err(|error| Blocked::WelesUnreachable {
            detail: format!("POST {base}/reauth: {error:?}"),
        })?;
    let status = response.status().as_u16();
    let answer: Value = response.json().await.map_err(|error| Blocked::Operation {
        code: "authentication_response_invalid".into(),
        stage: "weles_response".into(),
        detail: format!("Weles HTTP {status} returned invalid JSON: {error}"),
        status: Some(status),
    })?;
    if let Some(detail) = trajectory::refusal(&answer, status, &resolved.login_item) {
        let mut failure = answer.get("failure").filter(|value| value.is_object()).cloned().unwrap_or_else(|| json!({
            "code": answer.get("error").and_then(Value::as_str).unwrap_or("authentication_failed"),
            "stage": answer.get("stage").and_then(Value::as_str).unwrap_or("weles_execution"),
            "browser_started": false, "retryable": true,
        }));
        failure["run_id"] = answer.get("run_id").cloned().unwrap_or(Value::Null);
        failure["http_status"] = json!(status);
        return Ok(verdict(
            options,
            &identity,
            FAILED,
            detail,
            failure,
            Value::Null,
        ));
    }
    let refresh = pool::refresh_subscription(&options.provider, &id, &options.reason).await?;
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
