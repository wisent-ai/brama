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

/// The age, in whole seconds, before a second browser sign-in may be driven
/// for one stored credential. Unset, nothing but the verdict gate and the
/// host's declared `brama maintain` schedule paces sign-ins.
const SIGN_IN_COOLDOWN_ENV: &str = "BRAMA_CREDENTIAL_SIGN_IN_COOLDOWN_SECS";

/// Only one remote browser sign-in may own Weles at a time. OAuth refreshes
/// remain per-subscription and continue while this lock is held.
static SIGN_IN_SERIAL: LazyLock<tokio::sync::Mutex<()>> =
    LazyLock::new(|| tokio::sync::Mutex::new(()));

/// The declared cooldown, `None` when unset, refused by name when it is not
/// a whole number of seconds.
pub(crate) fn sign_in_cooldown() -> Result<Option<Duration>, String> {
    crate::types::declared_age(SIGN_IN_COOLDOWN_ENV)
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
        let logged_subscription = subscription_id.clone();
        let options = sign_in::SignInOptions {
            provider: provider.clone(),
            login_item: None,
            subscription_id: Some(subscription_id.clone()),
            reason: reason.clone(),
            // The gateway's log says where an automatic sign-in is while it
            // runs, and whose hand it waits for, not only how it ended.
            progress: Some(std::sync::Arc::new(move |event: &serde_json::Value| {
                if let Some(sentence) = sign_in::progress_sentence(event) {
                    info!(
                        event = "credential_sign_in_progress",
                        subscription = %logged_subscription,
                        %sentence
                    );
                }
            })),
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
