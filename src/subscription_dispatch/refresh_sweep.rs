//! Replace an access token before it dies, instead of learning it is dead from
//! a refused request.
//!
//! Brama used to refresh a grant at exactly one moment: inside a request, after
//! the local expiry said the token was spent or after the provider rejected it.
//! That is late in two ways. The request pays for the refresh, and -- worse --
//! nothing at all happens for a subscription no request reaches, so a credential
//! whose refresh token the provider had already disowned kept sitting in the
//! vault reading as active. Four of them did, for five days, while every request
//! that touched them failed and nothing said why.
//!
//! This module is the other moment: a short timer that walks every active
//! subscription, refreshes each grant that expires inside a skew window, and
//! records what the provider said about the ones it refuses. Four rules hold
//! here.
//!
//! A refresh is single-flighted per subscription, so a slow one is never
//! started twice; the rotation lock in the broker already serialises the writes,
//! and this is what stops a sweep from queueing behind its own previous attempt.
//! One subscription's failure -- including a panic -- ends that subscription's
//! turn and nothing else. A credential the provider has definitively refused is
//! left alone until a sign-in replaces it, because asking again every minute
//! cannot produce a different answer and would bury the one log line that
//! matters. And the ledger is consulted before the vault is: reading a
//! credential shells out to the entitlements router, so a grant whose recorded
//! expiry is hours away is skipped without reading anything at all.
//!
//! Nothing here spends provider quota: a token endpoint is not a metered
//! endpoint, so this sweep can run on a short timer without costing an account
//! anything it would otherwise have spent on a request.

use std::collections::{BTreeSet, HashSet};
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use tracing::{info, warn};

use crate::gateway::broker::{self, RefreshAhead};
use crate::subscription_dispatch::usage::{self, RefreshHint};

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

/// The subscriptions a refresh is running for right now.
static IN_FLIGHT: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(|| Mutex::new(HashSet::new()));

/// One subscription claimed for the duration of one refresh.
///
/// Held for the whole attempt and released by dropping, so a refresh that dies
/// on a panic or an early return does not leave its subscription claimed for the
/// life of the process.
struct InFlight(String);

impl InFlight {
    /// Claim this subscription, or nothing when a refresh for it is already
    /// running.
    fn claim(subscription_id: &str) -> Option<Self> {
        let mut claimed = match IN_FLIGHT.lock() {
            Ok(claimed) => claimed,
            Err(poisoned) => poisoned.into_inner(),
        };
        claimed
            .insert(subscription_id.to_owned())
            .then(|| Self(subscription_id.to_owned()))
    }
}

impl Drop for InFlight {
    fn drop(&mut self) {
        let mut claimed = match IN_FLIGHT.lock() {
            Ok(claimed) => claimed,
            Err(poisoned) => poisoned.into_inner(),
        };
        claimed.remove(&self.0);
    }
}

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

/// What one subscription's turn came to, so the sweep can say what it did
/// rather than only that it ran.
#[derive(Clone, Copy)]
enum Swept {
    /// This grant has more than the skew window left.
    NotDue,
    /// The access token was replaced before it expired.
    Refreshed,
    /// The refresh was refused, or the refreshed grant could not be stored.
    Refused,
    /// A refresh for this subscription was already running, or no credential
    /// could be read to look at.
    Skipped,
}

/// Walk every active subscription once, refreshing what is due.
async fn sweep(skew: Duration) {
    let mut visited = BTreeSet::new();
    let mut refreshed = usize::default();
    let mut refused = usize::default();
    let mut awaiting_signin = usize::default();
    for agent in broker::configured_request_sign_agents() {
        for entry in broker::list_subscriptions(&agent).await {
            if entry.status != "active" || crate::journal::is_retired(&entry.id) {
                continue;
            }
            // Only a provider whose credentials are OAuth grants has anything to
            // do here. An API key has no access token that expires, so reading
            // one to discover that would cost a vault read every minute and
            // learn nothing.
            if !broker::supports_oauth_refresh(&entry.provider) {
                continue;
            }
            // A subscription two agents share is one account with one grant, and
            // refreshing it twice would rotate a refresh token the first pass
            // has already replaced.
            if !visited.insert(entry.id.clone()) {
                continue;
            }
            // What the ledger already knows decides whether the vault is read at
            // all. A grant the provider has disowned is not retried until a
            // sign-in replaces it, because the answer cannot change and asking
            // every minute would cost one log line a minute per dead account. A
            // grant whose recorded expiry is hours away is not read either: the
            // read shells out to the entitlements router, and this file already
            // holds the number that read would return.
            match usage::credential_refresh_hint(&entry.id, skew) {
                RefreshHint::AwaitingSignIn => {
                    awaiting_signin = awaiting_signin.saturating_add(1);
                    continue;
                }
                RefreshHint::NotDue => continue,
                RefreshHint::Read => {}
            }
            match refresh_one(entry.id, entry.provider, skew).await {
                Swept::Refreshed => refreshed = refreshed.saturating_add(1),
                Swept::Refused => refused = refused.saturating_add(1),
                Swept::NotDue | Swept::Skipped => {}
            }
        }
    }
    info!(
        event = "credential_refresh_sweep_finished",
        subscriptions = visited.len(),
        refreshed,
        refused,
        awaiting_signin,
        "finished one credential refresh sweep"
    );
}

/// Refresh one subscription, surviving anything it does.
///
/// The work runs in its own task so that a panic below -- in a credential
/// parser, a provider client or the vault writer -- is a logged join error
/// against one subscription instead of the silent death of the whole timer.
async fn refresh_one(subscription_id: String, provider: String, skew: Duration) -> Swept {
    let logged_id = subscription_id.clone();
    let logged_provider = provider.clone();
    match tokio::spawn(refresh_subscription(subscription_id, provider, skew)).await {
        Ok(swept) => swept,
        Err(error) => {
            warn!(
                event = "credential_refresh_panicked",
                subscription = %logged_id,
                provider = %logged_provider,
                %error,
                "a credential refresh died; the remaining subscriptions are unaffected"
            );
            Swept::Skipped
        }
    }
}

/// Refresh one subscription's grant when it expires inside the skew window.
async fn refresh_subscription(subscription_id: String, provider: String, skew: Duration) -> Swept {
    let Some(_claim) = InFlight::claim(&subscription_id) else {
        info!(
            event = "credential_refresh_already_running",
            subscription = %subscription_id,
            provider = %provider,
            "a refresh for this subscription is still running; this sweep leaves it alone"
        );
        return Swept::Skipped;
    };
    match broker::refresh_subscription_credential_ahead(&subscription_id, &provider, skew).await {
        RefreshAhead::NotDue { expires_at_ms } => {
            // Recorded so a reader can say until when this grant is good, which
            // is the question the console could not answer at all. The ledger
            // ignores a record that changes nothing, so this does not make every
            // row look freshly observed once a minute.
            usage::record_credential_active(&subscription_id, &provider, expires_at_ms, false);
            Swept::NotDue
        }
        RefreshAhead::Refreshed { expires_at_ms } => {
            info!(
                event = "credential_refreshed_ahead",
                subscription = %subscription_id,
                provider = %provider,
                expires_at_ms,
                skew_secs = skew.as_secs(),
                "replaced an access token before it expired"
            );
            Swept::Refreshed
        }
        // Both refusals are already classified, logged and recorded where the
        // refresh happened, so that the forced refresh a rejected request
        // triggers reaches the same verdict as this sweep does.
        RefreshAhead::Refused(_) => Swept::Refused,
        RefreshAhead::Unavailable(refused) => {
            warn!(
                event = "credential_refresh_unavailable",
                subscription = %subscription_id,
                provider = %provider,
                error = refused.detail.as_deref().unwrap_or_default(),
                envelope = %refused.to_json(),
                "no credential could be read for this subscription, so nothing about its \
                 grant is known"
            );
            Swept::Skipped
        }
    }
}
