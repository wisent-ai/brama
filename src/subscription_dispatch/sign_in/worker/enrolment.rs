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

use serde_json::{json, Value};

use super::super::blocked::SignInError;
use super::account::{resolve, HTTP_OK, LOGIN_ITEM_SELECTOR};
use super::api::{worker_api_base, worker_api_token};

/// What one enrolment came to, in the words an operator repairs it with.
pub struct Enrolment {
    pub subscription_id: String,
    pub login_item: String,
    /// The Weles run that answered, when its answer named one.
    pub run_id: Option<String>,
    /// Whether Skarbiec reports a usable seed on that login afterwards.
    pub seed_present: bool,
    /// Weles confirmed this exact account revision and a successful run.
    pub confirmed: bool,
    pub detail: String,
}

impl Enrolment {
    /// Weles confirmed the enrolment and Skarbiec holds a usable seed for it.
    pub fn enrolled(&self) -> bool {
        self.confirmed && self.seed_present
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
) -> Result<Enrolment, SignInError> {
    let base = worker_api_base()
        .await
        .map_err(SignInError::Dependency)?
        .url;
    let token = worker_api_token().map_err(SignInError::Dependency)?;
    let client = reqwest::Client::builder()
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
    let response = stado_wait::request(
        client
            .post(format!("{base}/reauth/enrol-authenticator"))
            .bearer_auth(&token)
            .json(&json!({
                "provider": weles_provider,
                "subscription_id": subscription_id,
                "login_item": resolved.login_item,
                "account_revision": resolved.account_revision,
            })),
    )
    .await
    .map_err(|error| {
        SignInError::Dependency(format!(
            "POST {base}/reauth/enrol-authenticator is unconfirmed: {error:?}"
        ))
    })?;
    let status = response.status().as_u16();
    let answer: Value = response.json().await.map_err(|error| {
        SignInError::Dependency(format!(
            "Weles HTTP {status} returned an unreadable enrolment result: {error}"
        ))
    })?;
    // The run Weles names, when it names one: a run id is what an operator
    // reads the run back by, and an answer without one names none.
    let run_id = answer
        .get("run_id")
        .and_then(Value::as_str)
        .filter(|run| !run.is_empty())
        .map(str::to_string);
    let claimed = status == HTTP_OK
        && answer.get("ok").and_then(Value::as_bool) == Some(true)
        && answer.get(LOGIN_ITEM_SELECTOR).and_then(Value::as_str)
            == Some(resolved.login_item.as_str())
        && answer.get("subscription_id").and_then(Value::as_str)
            == Some(resolved.subscription_id.as_str())
        && answer.get("account_revision").and_then(Value::as_str)
            == Some(resolved.account_revision.as_str())
        && answer.get("source_revision").and_then(Value::as_str)
            == Some(resolved.source_revision.as_str())
        && run_id.is_some();
    let run = match &run_id {
        Some(run) => format!("Weles run {run}"),
        None => "Weles named no run".to_string(),
    };
    // The seed check reads the vault this machine runs Skarbiec against. On
    // a workstation that read fails ("this machine holds no Skarbiec vault"),
    // and failing the whole command on it threw away Weles's answer: a
    // 21-minute enrolment ended with neither its run id nor its outcome. The
    // read's failure is now reported beside what Weles said.
    let login = resolved.login_item.as_str();
    let (seed_present, unread) = match crate::gateway::broker::login_seed_present(login) {
        Ok(present) => (present, String::new()),
        Err(error) => (
            false,
            format!(
                "; {run}; whether Skarbiec holds the seed was not confirmed here ({error}); \
                 ask the vault owner: stado credentials seed-freshness --host <vault owner> \
                 --login-item {login}"
            ),
        ),
    };
    let detail = if claimed && seed_present {
        format!(
            "Weles confirmed authenticator enrolment for {login} and Skarbiec holds its seed; \
             later {provider} sign-ins can answer authenticator challenges"
        )
    } else if claimed {
        format!(
            "Weles reported enrolment for {login}, but Skarbiec did not confirm a usable seed \
             on that login"
        )
    } else {
        // Weles's enrolment route states why a run was not confirmed in two
        // fields of its own, `blocked` and `message`, built from the run's
        // result. The rest of the answer spreads the enrolment run's output,
        // which handles the seed, so it is never printed: an answer without
        // the two fields is said by the names of the fields it carried.
        let subscription = &resolved.subscription_id;
        let blocked = answer.get("blocked").and_then(Value::as_str);
        let message = answer.get("message").and_then(Value::as_str);
        match (blocked, message) {
            (Some(blocked), Some(message)) => format!(
                "{run} answered HTTP {status} without confirming the enrolment of {login} for \
                 subscription {subscription}: {blocked}: {message}"
            ),
            (None, _) | (Some(_), None) => {
                let carried = match answer.as_object() {
                    Some(object) => format!(
                        "it carried the fields [{}]",
                        object
                            .keys()
                            .map(String::as_str)
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                    None => "its answer is not a JSON object".to_string(),
                };
                format!(
                    "{run} answered HTTP {status} without confirming the enrolment of {login} \
                     for subscription {subscription}, and without the blocked and message \
                     fields its route states why in; {carried}"
                )
            }
        }
    };
    let detail = detail + &unread;
    Ok(Enrolment {
        subscription_id: resolved.subscription_id,
        login_item: resolved.login_item,
        run_id,
        seed_present,
        confirmed: claimed,
        detail,
    })
}
