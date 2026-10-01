//! Driving one browser sign-in for a subscription whose stored credential can
//! no longer be renewed silently.
//!
//! A silent OAuth refresh and a browser sign-in are different acts. The first
//! costs a call to a token endpoint and can run for every subscription at once;
//! the second borrows a real human account, can only be spent one at a time,
//! and is remembered by the provider whether or not it succeeded. So everything
//! that guards the account is here rather than in the walk: the verdict gate
//! that refuses to ask a question already answered, the restart-safe cooldown
//! the journal supplies, the claim that keeps two runs off one subscription,
//! and the serial lock that lets only one browser own Weles. The sweep decides
//! which subscription needs a sign-in; this decides whether one may happen and
//! what is recorded when it ends.

use std::sync::LazyLock;
use std::time::Duration;

use tracing::{info, warn};

use crate::subscription_dispatch::sign_in;

use super::claim::InFlight;

const SIGN_IN_COOLDOWN_ENV: &str = "BRAMA_CREDENTIAL_SIGN_IN_COOLDOWN_SECS";
const SIGN_IN_TIMEOUT_ENV: &str = "BRAMA_CREDENTIAL_SIGN_IN_TIMEOUT_MS";
const DEFAULT_SIGN_IN_COOLDOWN_SECS: u64 = 30 * 60;
const DEFAULT_SIGN_IN_TIMEOUT_MS: u64 = 15 * 60 * 1000;

/// Only one remote browser sign-in may own Weles at a time. OAuth refreshes
/// remain per-subscription and continue while this lock is held.
static SIGN_IN_SERIAL: LazyLock<tokio::sync::Mutex<()>> =
    LazyLock::new(|| tokio::sync::Mutex::new(()));

pub(crate) fn sign_in_cooldown() -> Duration {
    let seconds = std::env::var(SIGN_IN_COOLDOWN_ENV)
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .filter(|seconds| *seconds > 0)
        .unwrap_or(DEFAULT_SIGN_IN_COOLDOWN_SECS);
    Duration::from_secs(seconds)
}

fn sign_in_timeout_ms() -> u64 {
    std::env::var(SIGN_IN_TIMEOUT_ENV)
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .filter(|timeout| *timeout > 0)
        .unwrap_or(DEFAULT_SIGN_IN_TIMEOUT_MS)
}

/// Start one account sign-in without making the refresh sweep wait for a
/// browser. The claim is acquired before spawning, and the completed journal
/// record supplies a restart-safe cooldown.
///
/// Every path that can drive a browser funnels through here, so the verdict
/// gate belongs here and nowhere else.
pub(super) fn schedule_sign_in(subscription_id: String, provider: String) -> bool {
    let Some(claim) = InFlight::claim(&subscription_id) else {
        return false;
    };
    tokio::spawn(async move {
        let _claim = claim;
        let _serial = SIGN_IN_SERIAL.lock().await;
        let reason = "automatic OAuth credential renewal".to_owned();
        let options = sign_in::SignInOptions {
            provider: provider.clone(),
            login_item: None,
            subscription_id: Some(subscription_id.clone()),
            reason: reason.clone(),
            login_timeout_ms: sign_in_timeout_ms(),
        };
        match tokio::spawn(sign_in::sign_in_provider(options)).await {
            Ok(Ok(verdict)) => {
                info!(
                    event = "credential_sign_in_finished",
                    subscription = %subscription_id,
                    provider = %provider,
                    result = verdict.get("result").and_then(serde_json::Value::as_str).unwrap_or("unknown"),
                    detail = verdict.get("detail").and_then(serde_json::Value::as_str).unwrap_or_default()
                );
                if verdict.to_string().contains(MISSING_SEED) {
                    enrol_missing_seed(&subscription_id, &provider).await;
                }
            }
            // A blocked reason is a declaration this deployment is missing: it
            // will read the same next minute, so it is logged with its own
            // word and envelope rather than as a passing dependency failure.
            // Neither writes the sign-in cooldown, because nothing
            // account-sensitive happened: no browser ran and Weles accepted
            // no account.
            Ok(Err(error)) => {
                let blocked = error.blocked();
                warn!(
                    event = if blocked.is_some() {
                        "credential_sign_in_blocked"
                    } else {
                        "credential_sign_in_preflight_failed"
                    },
                    subscription = %subscription_id,
                    provider = %provider,
                    blocked_by = blocked.map(super::super::sign_in::Blocked::code).unwrap_or("none"),
                    envelope = blocked
                        .map(|blocked| blocked.failure(Some(&subscription_id)).to_string())
                        .unwrap_or_default(),
                    detail = %error
                );
                // A missing seed arrives here as `credential_sign_in_blocked`
                // with `google_2fa_material_missing`.
                if error.to_string().contains(MISSING_SEED)
                    || blocked.is_some_and(|blocked| blocked.code() == MISSING_SEED)
                {
                    enrol_missing_seed(&subscription_id, &provider).await;
                }
            }
            // A failed join likewise proves no completed Weles verdict. Keeping
            // it out of the journal prevents a transient process fault from
            // suppressing renewal for the full account cooldown.
            Err(error) => {
                let detail = format!("automatic sign-in task failed: {error}");
                warn!(
                    event = "credential_sign_in_panicked",
                    subscription = %subscription_id,
                    provider = %provider,
                    %detail
                );
            }
        }
    });
    true
}

/// Weles's word for a Google login whose Skarbiec item holds no
/// authenticator seed, so its sign-in stops at the second-factor screen.
const MISSING_SEED: &str = "google_2fa_material_missing";
/// How long one subscription waits before its enrolment is ordered again.
/// Each enrolment pages the operator for one approval on the phone.
const ENROL_COOLDOWN: Duration = Duration::from_secs(6 * 60 * 60);
/// The same bound the sign-in gives the operator to approve Google's push.
const ENROL_TIMEOUT_MS: u64 = 15 * 60 * 1000;

static ENROLLED_AT: LazyLock<
    std::sync::Mutex<std::collections::HashMap<String, std::time::Instant>>,
> = LazyLock::new(Default::default);

/// Order the authenticator enrolment the sign-in's own refusal names.
///
/// Only `brama subscription enrol-authenticator` used to order it, so every
/// automatic sign-in of a Google login without a seed stopped at
/// `google_2fa_material_missing` and waited for somebody to type that
/// command: on this fleet 0 of 15 subscriptions were live for a week and no
/// enrolment ever asked the operator for the one approval it needs. The sweep
/// now orders it itself, still holding the serial lock so only one browser
/// owns Weles, at most once per subscription every six hours.
async fn enrol_missing_seed(subscription_id: &str, provider: &str) {
    {
        let mut enrolled = ENROLLED_AT
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if enrolled
            .get(subscription_id)
            .is_some_and(|at| at.elapsed() < ENROL_COOLDOWN)
        {
            return;
        }
        enrolled.insert(subscription_id.to_owned(), std::time::Instant::now());
    }
    let Some(weles) = sign_in::weles_provider(provider) else {
        return;
    };
    match sign_in::enrol_authenticator(provider, weles, subscription_id, None, ENROL_TIMEOUT_MS)
        .await
    {
        Ok(enrolment) => info!(
            event = "credential_authenticator_enrolment",
            subscription = %subscription_id,
            provider = %provider,
            login_item = %enrolment.login_item,
            run = %enrolment.run_id,
            seed_present = enrolment.seed_present,
            detail = %enrolment.detail
        ),
        Err(error) => warn!(
            event = "credential_authenticator_enrolment_failed",
            subscription = %subscription_id,
            provider = %provider,
            detail = %error
        ),
    }
}
