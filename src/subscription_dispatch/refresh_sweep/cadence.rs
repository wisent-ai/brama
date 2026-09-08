//! How often the sweep runs, how far ahead of expiry it replaces a grant, and
//! how long it waits before its first pass.
//!
//! These three numbers decide whether an access token is ever handed to a
//! request that outlives it, and all three are decisions about the host rather
//! than about the product: a machine that must not refresh in the background
//! says so with a zero, and a gateway that starts with a dead credential should
//! say so in its first minute. They live apart from the walk so that reading
//! the sweep does not mean reading environment variables first, and so that the
//! off switch -- and the one log line that admits it is off -- is in a single
//! place.

use std::time::Duration;

use tracing::info;

use super::sweep;

const INTERVAL_ENV: &str = "BRAMA_CREDENTIAL_REFRESH_INTERVAL_SECS";
const SKEW_ENV: &str = "BRAMA_CREDENTIAL_REFRESH_SKEW_SECS";
/// One minute: short enough that a token with a five-minute skew window is
/// refreshed several sweeps before it expires even if some of those sweeps are
/// slow, and cheap enough that the cost is a handful of vault reads a minute.
const DEFAULT_INTERVAL_SECS: u64 = 60;
/// Five minutes ahead of expiry. A token replaced this early is never handed to
/// a request that outlives it, which is the failure this window exists to
/// prevent: a token valid when the request was dispatched and expired when the
/// provider read it.
const DEFAULT_SKEW_SECS: u64 = 5 * 60;
/// Long enough for the listener to be bound and the entitlements router to have
/// answered its first read. Deliberately shorter than the usage probe's delay:
/// a gateway that starts with a dead credential should say so in the first
/// minute, not after it has refused requests for half an hour.
const STARTUP_DELAY_SECS: u64 = 5;

/// How often to sweep every subscription, or `None` when refreshing ahead is
/// off.
fn interval() -> Option<Duration> {
    let seconds = std::env::var(INTERVAL_ENV)
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .unwrap_or(DEFAULT_INTERVAL_SECS);
    // Zero is the documented off switch rather than a busy loop, the same
    // convention the usage probe uses: a host that must not refresh in the
    // background says so with a number.
    (seconds > 0).then(|| Duration::from_secs(seconds))
}

/// How far ahead of expiry a grant is replaced.
fn skew() -> Duration {
    let seconds = std::env::var(SKEW_ENV)
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .filter(|seconds| *seconds > 0)
        .unwrap_or(DEFAULT_SKEW_SECS);
    Duration::from_secs(seconds)
}

/// Start refreshing credentials ahead of expiry, unless this host turned it off.
pub fn spawn() {
    let Some(period) = interval() else {
        info!(
            event = "credential_refresh_sweep_disabled",
            env = INTERVAL_ENV,
            "credentials are not refreshed ahead of expiry; a grant is refreshed only \
             when a request needs it and a refused grant stays unreported until one does"
        );
        return;
    };
    let skew = skew();
    info!(
        event = "credential_refresh_sweep_scheduled",
        interval_secs = period.as_secs(),
        skew_secs = skew.as_secs(),
        "refreshing subscription credentials ahead of expiry"
    );
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(STARTUP_DELAY_SECS)).await;
        loop {
            sweep(skew).await;
            tokio::time::sleep(period).await;
        }
    });
}
