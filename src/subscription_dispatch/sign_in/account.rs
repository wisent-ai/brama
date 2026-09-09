use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::blocked::{Blocked, SignInError};

pub(super) const LOGIN_ITEM_SELECTOR: &str = "login_item";

/// Only opaque vault coordinates and non-secret account provenance cross here.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub(super) struct ResolvedAccount {
    pub subscription_id: String,
    pub subscription_item: String,
    pub login_item: String,
    pub provider: String,
    pub account_ref: String,
    pub login_method: String,
    pub source_revision: String,
}

pub(super) async fn resolve(
    client: &reqwest::Client,
    base: &str,
    token: &str,
    provider: &str,
    subscription_id: &str,
    requested_login: Option<&str>,
) -> Result<ResolvedAccount, SignInError> {
    let response = client
        .post(format!("{base}/reauth/resolve"))
        .bearer_auth(token)
        .json(
            &json!({"provider": provider, "subscription_id": subscription_id,
            "login_item": requested_login}),
        )
        .send()
        .await
        .map_err(|error| Blocked::WelesUnreachable {
            detail: format!("POST {base}/reauth/resolve: {error:?}"),
        })?;
    let status = response.status().as_u16();
    let answer: Value = response.json().await.map_err(|error| Blocked::Operation {
        code: "account_resolution_response_invalid".into(),
        stage: "identity".into(),
        detail: format!("POST {base}/reauth/resolve returned invalid JSON: {error}"),
        status: Some(status),
    })?;
    if status != 200 || answer.get("ok").and_then(Value::as_bool) != Some(true) {
        return Err(Blocked::Operation {
            code: answer
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("account_resolution_failed")
                .into(),
            stage: "identity".into(),
            status: Some(status),
            detail: answer
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("Weles could not resolve the subscription from Skarbiec")
                .into(),
        }
        .into());
    }
    let resolved: ResolvedAccount =
        serde_json::from_value(answer).map_err(|error| Blocked::Operation {
            code: "account_resolution_response_invalid".into(),
            stage: "identity".into(),
            detail: format!("Weles account resolution omitted a required identity field: {error}"),
            status: Some(status),
        })?;
    if resolved.subscription_id != subscription_id
        || resolved.provider != provider
        || resolved.subscription_item.is_empty()
        || resolved.login_item.is_empty()
        || resolved.account_ref.is_empty()
        || resolved.source_revision.is_empty()
    {
        return Err(Blocked::Operation {
            code: "subscription_identity_mismatch".into(), stage: "identity".into(),
            detail: format!("Weles resolved a different or incomplete identity for subscription {subscription_id}"),
            status: Some(status),
        }.into());
    }
    Ok(resolved)
}
