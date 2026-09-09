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
use crate::subscription_dispatch::usage;

use super::claim::InFlight;
use super::verdict::verdict_outranks_last_sign_in;

const SIGN_IN_COOLDOWN_ENV: &str = "BRAMA_CREDENTIAL_SIGN_IN_COOLDOWN_SECS";
const SIGN_IN_TIMEOUT_ENV: &str = "BRAMA_CREDENTIAL_SIGN_IN_TIMEOUT_MS";
const DEFAULT_SIGN_IN_COOLDOWN_SECS: u64 = 30 * 60;
const DEFAULT_SIGN_IN_TIMEOUT_MS: u64 = 15 * 60 * 1000;

/// Only one remote browser sign-in may own Weles at a time. OAuth refreshes
/// remain per-subscription and continue while this lock is held.
static SIGN_IN_SERIAL: LazyLock<tokio::sync::Mutex<()>> =
    LazyLock::new(|| tokio::sync::Mutex::new(()));

fn sign_in_cooldown() -> Duration {
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
pub(super) fn schedule_sign_in(
    subscription_id: String,
    provider: String,
    login_item: Option<String>,
) -> bool {
    if !verdict_outranks_last_sign_in(
        usage::credential_recorded_at_ms(&subscription_id),
        crate::journal::latest_subscription_sign_in_at_ms(&subscription_id),
    ) {
        warn!(
            event = "credential_sign_in_withheld",
            subscription = %subscription_id,
            provider = %provider,
            "a browser sign-in has already been driven against this exact stored credential; \
             only replacing it changes the answer, so this one is left to an operator"
        );
        return false;
    }
    let cooldown = sign_in_cooldown();
    if !crate::journal::subscription_sign_in_due(&subscription_id, cooldown) {
        return false;
    }
    let Some(claim) = InFlight::claim(&subscription_id) else {
        return false;
    };
    // Old primary subscriptions may predate the login tag. Weles declares one
    // primary account per provider; the first successful donation writes that
    // exact account back as `brama:login:`, completing the migration without a
    // one-off vault helper.
    let login_item = login_item.filter(|item| !item.trim().is_empty());
    let login_label = login_item
        .as_deref()
        .unwrap_or("Weles-declared primary")
        .to_owned();
    tokio::spawn(async move {
        let _claim = claim;
        let _serial = SIGN_IN_SERIAL.lock().await;
        let reason = "automatic OAuth credential renewal".to_owned();
        let options = sign_in::SignInOptions {
            provider: provider.clone(),
            login_item: login_item.clone(),
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
                    login_item = %login_label,
                    result = verdict.get("result").and_then(serde_json::Value::as_str).unwrap_or("unknown"),
                    detail = verdict.get("detail").and_then(serde_json::Value::as_str).unwrap_or_default()
                );
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
                    login_item = %login_label,
                    blocked_by = blocked.map(super::super::sign_in::Blocked::code).unwrap_or("none"),
                    envelope = blocked
                        .map(|blocked| blocked.failure(Some(&subscription_id)).to_string())
                        .unwrap_or_default(),
                    detail = %error
                );
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
                    login_item = %login_label,
                    %detail
                );
            }
        }
    });
    true
}
