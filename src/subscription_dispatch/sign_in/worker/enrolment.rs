//! Asking Weles to enrol an authenticator for the login behind one
//! subscription, and confirming the seed landed.
//!
//! This exists because of one refusal. An automatic sign-in stops at
//! `google_2fa_material_missing` when the Skarbiec login carries no
//! authenticator seed, and no sign-in can invent one: Google shows a setup key
//! only inside a signed-in session. Weles has the trajectory that does it —
//! one approval on the operator's phone, then it reads the key, proves the
//! first code and writes it to the login item — and until now Brama could not
//! order that trajectory, because reaching it meant holding Weles's general
//! worker token. So the product that reports the refusal could not order its
//! own repair, and the repair happened by a person pasting a one-time code:
//! that fixes one grant and leaves the next sign-in stopping in the same
//! place. `POST /reauth/enrol-authenticator` is admitted by the same bearer
//! `/reauth` is, and this is the call to it.
//!
//! The seed is never read here. Whether it landed is asked of Skarbiec, which
//! reports the state of the field without disclosing it, because a trajectory
//! that answers `ok` is a claim and the vault is the world.

use std::time::Duration;

use serde_json::{json, Value};

use super::super::blocked::SignInError;
use super::account::{resolve, HTTP_OK, LOGIN_ITEM_SELECTOR};
use super::api::{transport_timeout_seconds, worker_api_base, worker_api_token};

/// What one enrolment came to, in the words an operator repairs it with.
pub struct Enrolment {
    pub subscription_id: String,
    pub login_item: String,
    pub run_id: String,
    /// Whether Skarbiec reports a usable seed on that login afterwards.
    pub seed_present: bool,
    pub detail: String,
}

impl Enrolment {
    pub fn ok(&self) -> bool {
        self.seed_present
    }
}

/// Enrol an authenticator for the login behind one subscription.
///
/// The provider is Brama's own name for it (`claude-code`, `codex`, `kimi`);
/// Weles resolves which vault row that subscription means, exactly as it does
/// for a sign-in, so nothing here chooses a credential.
pub async fn enrol_authenticator(
    provider: &str,
    weles_provider: &str,
    subscription_id: &str,
    login_item: Option<&str>,
    timeout_ms: u64,
) -> Result<Enrolment, SignInError> {
    let base = worker_api_base()
        .await
        .map_err(SignInError::Dependency)?
        .url;
    let token = worker_api_token().map_err(SignInError::Dependency)?;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(transport_timeout_seconds()))
        .build()
        .map_err(|error| SignInError::Dependency(format!("Weles HTTP client: {error}")))?;
    let resolved = resolve(
        &client,
        &base,
        &token,
        weles_provider,
        subscription_id,
        login_item,
    )
    .await?;
    let response = client
        .post(format!("{base}/reauth/enrol-authenticator"))
        .bearer_auth(&token)
        .json(&json!({
            "provider": weles_provider,
            "subscription_id": subscription_id,
            "login_item": resolved.login_item,
            "timeout_ms": timeout_ms,
        }))
        .send()
        .await
        .map_err(|error| {
            SignInError::Dependency(format!(
                "POST {base}/reauth/enrol-authenticator is unconfirmed: {error}"
            ))
        })?;
    let status = response.status().as_u16();
    let answer: Value = response.json().await.map_err(|error| {
        SignInError::Dependency(format!(
            "Weles HTTP {status} returned an unreadable enrolment result: {error}"
        ))
    })?;
    let run_id = answer
        .get("run_id")
        .and_then(Value::as_str)
        .unwrap_or("unreported")
        .to_string();
    let echoed = answer.get(LOGIN_ITEM_SELECTOR).and_then(Value::as_str);
    let claimed = status == HTTP_OK
        && answer.get("ok").and_then(Value::as_bool) == Some(true)
        && echoed == Some(resolved.login_item.as_str());
    let seed_present = crate::gateway::broker::login_seed_present(&resolved.login_item);
    let detail = if claimed && seed_present {
        format!(
            "Weles enrolled an authenticator for {} and Skarbiec now holds its seed; every later sign-in for {provider} answers Google itself",
            resolved.login_item
        )
    } else if claimed {
        format!(
            "Weles reported an enrolment for {}, and Skarbiec reports no usable seed on that login: the run wrote nothing that answers a second factor",
            resolved.login_item
        )
    } else {
        let said = ["error", "message", "blocked", "stderr_tail"]
            .iter()
            .filter_map(|field| answer.get(field).and_then(Value::as_str))
            .filter(|value| !value.trim().is_empty())
            .collect::<Vec<_>>()
            .join(" | ");
        let said: String = said.chars().take(1800).collect();
        format!(
            "Weles enrolment run {run_id} answered HTTP {status}, login_item={}; {}",
            echoed.unwrap_or("unreported"),
            if said.is_empty() {
                "the trajectory reported no reason"
            } else {
                said.as_str()
            }
        )
    };
    Ok(Enrolment {
        subscription_id: resolved.subscription_id,
        login_item: resolved.login_item,
        run_id,
        seed_present,
        detail,
    })
}
