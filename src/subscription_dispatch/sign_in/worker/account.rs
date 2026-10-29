use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::super::blocked::{Blocked, SignInError};

pub(crate) const LOGIN_ITEM_SELECTOR: &str = "login_item";
/// Weles answers a finished resolution or trajectory with 200 OK and `ok: true`.
pub(in crate::subscription_dispatch::sign_in) const HTTP_OK: u16 = reqwest::StatusCode::OK.as_u16();

/// Only opaque vault coordinates and non-secret account provenance cross here.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub(crate) struct ResolvedAccount {
    pub subscription_id: String,
    pub subscription_item: String,
    pub login_item: String,
    pub provider: String,
    pub account_ref: String,
    pub login_method: String,
    pub source_revision: String,
    pub account_revision: String,
}

pub(crate) async fn resolve(
    client: &reqwest::Client,
    base: &str,
    token: &str,
    provider: &str,
    subscription_id: &str,
    requested_login: Option<&str>,
) -> Result<ResolvedAccount, SignInError> {
    let response = stado_wait::request(
        client
            .post(format!("{base}/reauth/resolve"))
            .bearer_auth(token)
            .json(
                &json!({"provider": provider, "subscription_id": subscription_id,
                "login_item": requested_login}),
            ),
    )
    .await
    .map_err(|error| Blocked::WelesUnreachable {
        detail: format!(
            "Weles did not answer POST {base}/reauth/resolve, so no sign-in was started: {error:?}"
        ),
    })?;
    let status = response.status().as_u16();
    // The body is read as text first so an undecodable answer is reported
    // with what Weles (or the Stado adapter in front of it) actually sent.
    let body = response.text().await.map_err(|error| Blocked::Operation {
        code: "account_resolution_response_invalid".into(),
        stage: "identity".into(),
        detail: format!("POST {base}/reauth/resolve HTTP {status}: body unreadable: {error}"),
        status: Some(status),
    })?;
    let answer: Value = match serde_json::from_str(&body) {
        Ok(answer) => answer,
        // A server error whose body is not JSON is the route in front of
        // Weles saying Weles did not answer (a Stado adapter's `502 upstream
        // unavailable`): the dependency is down, not the account's identity
        // wrong, and the next maintenance pass asks again.
        Err(error)
            if reqwest::StatusCode::from_u16(status).is_ok_and(|code| code.is_server_error()) =>
        {
            return Err(Blocked::WelesUnreachable {
                detail: format!(
                    "POST {base}/reauth/resolve answered HTTP {status} with no JSON ({error}), \
                     so Weles behind it did not answer: {}",
                    body.trim()
                ),
            }
            .into())
        }
        Err(error) => {
            return Err(Blocked::Operation {
                code: "account_resolution_response_invalid".into(),
                stage: "identity".into(),
                detail: format!(
                    "POST {base}/reauth/resolve HTTP {status} returned invalid JSON ({error}): {}",
                    body.trim()
                ),
                status: Some(status),
            }
            .into())
        }
    };
    if status != HTTP_OK || answer.get("ok").and_then(Value::as_bool) != Some(true) {
        // A refusal that names no code or message is reported with what Weles
        // sent, not with a sentence written here on its behalf.
        let code = match answer.get("error").and_then(Value::as_str) {
            Some(code) => code.to_string(),
            None => "account_resolution_refused_unnamed".to_string(),
        };
        let detail = match answer.get("message").and_then(Value::as_str) {
            Some(message) => message.to_string(),
            None => format!(
                "POST {base}/reauth/resolve HTTP {status} refused without a message: {}",
                body.trim()
            ),
        };
        return Err(Blocked::Operation {
            code,
            stage: "identity".into(),
            status: Some(status),
            detail,
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
        || resolved.account_revision.is_empty()
    {
        return Err(Blocked::Operation {
            code: "subscription_identity_mismatch".into(), stage: "identity".into(),
            detail: format!("Weles resolved a different or incomplete identity for subscription {subscription_id}"),
            status: Some(status),
        }.into());
    }
    Ok(resolved)
}
