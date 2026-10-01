//! How far ahead of expiry the sweep replaces a grant.
//!
//! This number decides whether an access token is ever handed to a request
//! that outlives it, and it is a decision about the host rather than about the
//! product. When the sweep runs is not decided here: `brama maintain` runs one
//! pass, and the host's Stado schedule says how often.

use std::time::Duration;

const SKEW_ENV: &str = "BRAMA_CREDENTIAL_REFRESH_SKEW_SECS";
/// Five minutes ahead of expiry. A token replaced this early is never handed to
/// a request that outlives it, which is the failure this window exists to
/// prevent: a token valid when the request was dispatched and expired when the
/// provider read it.
const DEFAULT_SKEW_SECS: u64 = 5 * 60;

/// How far ahead of expiry a grant is replaced.
pub(super) fn skew() -> Duration {
    let seconds = std::env::var(SKEW_ENV)
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .filter(|seconds| *seconds > 0)
        .unwrap_or(DEFAULT_SKEW_SECS);
    Duration::from_secs(seconds)
}
