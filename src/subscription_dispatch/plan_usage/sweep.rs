//! When Brama goes looking for a plan reading that has aged out, and what one
//! refused account costs the accounts after it.
//!
//! Nothing here talks to a provider, which is why it is its own file: this is
//! a timer, the operator's off switch for that timer, a startup delay, and the
//! rule that a subscription two agents share is still one account with one
//! plan and is visited once. Whether a visit becomes a request is the cache
//! window's decision, not this file's; the interval only decides how finely
//! those windows are noticed.

use std::collections::BTreeSet;
use std::time::Duration;

use tracing::{info, warn};

use super::refresh;
use crate::gateway::broker;
use crate::subscription_dispatch::usage;

const SWEEP_INTERVAL_ENV: &str = "BRAMA_PLAN_USAGE_SWEEP_SECS";
/// How often to look for subscriptions whose report has aged out.
///
/// A minute, which is not how often a report is read: each subscription is read
/// at most once per its own cache window. This is only how finely those windows
/// are noticed, and a minute is fine enough that a five-minute window is never
/// overshot by more than a minute.
const DEFAULT_SWEEP_INTERVAL_SECS: u64 = 60;
/// Long enough for the listener to be bound and the entitlements router to have
/// answered its first read, so the first sweep measures the gateway's steady
/// state rather than its startup.
const STARTUP_DELAY_SECS: u64 = 10;

/// How often to sweep, or `None` when this host has turned the sweep off.
fn sweep_interval() -> Option<Duration> {
    let seconds = std::env::var(SWEEP_INTERVAL_ENV)
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .unwrap_or(DEFAULT_SWEEP_INTERVAL_SECS);
    // Zero is the documented off switch rather than a busy loop: a host that
    // wants no background provider traffic at all says so with a number.
    (seconds > 0).then(|| Duration::from_secs(seconds))
}

/// Start keeping every subscription's plan reading current.
pub fn spawn() {
    let Some(period) = sweep_interval() else {
        info!(
            event = "plan_usage_sweep_disabled",
            env = SWEEP_INTERVAL_ENV,
            "provider usage reports will not be read on a timer; plan windows will only be \
             recorded for subscriptions that serve real traffic"
        );
        return;
    };
    info!(
        event = "plan_usage_sweep_scheduled",
        interval_secs = period.as_secs(),
        ttl_ms = usage::plan_usage_ttl_ms(),
        retention_ms = usage::plan_usage_retention_ms(),
        "reading provider usage reports on a timer; no completion is spent"
    );
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(STARTUP_DELAY_SECS)).await;
        loop {
            sweep().await;
            tokio::time::sleep(period).await;
        }
    });
}

/// Read every active subscription whose report has aged past its own window.
async fn sweep() {
    let mut seen = BTreeSet::new();
    let mut read = 0_usize;
    for agent in broker::configured_request_sign_agents() {
        for entry in broker::list_subscriptions(&agent).await {
            if entry.status != "active" || crate::journal::is_retired(&entry.id) {
                continue;
            }
            // A subscription shared by two agents is one account with one plan,
            // and reading it twice would ask one provider the same question
            // twice in one second.
            if !seen.insert(entry.id.clone()) {
                continue;
            }
            if !usage::plan_usage_due(&entry.id) {
                continue;
            }
            refresh_isolated(entry.id, entry.provider).await;
            read = read.saturating_add(1);
        }
    }
    info!(
        event = "plan_usage_swept",
        subscriptions = seen.len(),
        read,
        "finished one provider usage report sweep"
    );
}

/// Read one subscription's report without allowing a panic to stop the sweep.
async fn refresh_isolated(subscription_id: String, provider: String) {
    if let Err(error) = refresh(&subscription_id, &provider).await {
        warn!(
            event = "plan_usage_refresh_failed",
            subscription = %subscription_id,
            provider = %provider,
            envelope = %error.to_json(),
            "reading one provider usage report failed; the remaining subscriptions are unaffected"
        );
    }
}
