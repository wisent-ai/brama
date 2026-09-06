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
const SIGN_IN_COOLDOWN_ENV: &str = "BRAMA_CREDENTIAL_SIGN_IN_COOLDOWN_SECS";
const SIGN_IN_TIMEOUT_ENV: &str = "BRAMA_CREDENTIAL_SIGN_IN_TIMEOUT_MS";
/// Browser sign-in is separate from silent OAuth token renewal.
const AUTOMATIC_SIGN_IN_ENV: &str = "BRAMA_CREDENTIAL_AUTOMATIC_SIGN_IN";
/// One minute: short enough that a token with a five-minute skew window is
/// refreshed several sweeps before it expires even if some of those sweeps are
/// slow, and cheap enough that the cost is a handful of vault reads a minute.
const DEFAULT_INTERVAL_SECS: u64 = 60;
/// Five minutes ahead of expiry. A token replaced this early is never handed to
/// a request that outlives it, which is the failure this window exists to
/// prevent: a token valid when the request was dispatched and expired when the
/// provider read it.
const DEFAULT_SKEW_SECS: u64 = 5 * 60;
const DEFAULT_SIGN_IN_COOLDOWN_SECS: u64 = 30 * 60;
const DEFAULT_SIGN_IN_TIMEOUT_MS: u64 = 15 * 60 * 1000;
/// Long enough for the listener to be bound and the entitlements router to have
/// answered its first read. Deliberately shorter than the usage probe's delay:
/// a gateway that starts with a dead credential should say so in the first
/// minute, not after it has refused requests for half an hour.
const STARTUP_DELAY_SECS: u64 = 5;

/// The subscriptions a refresh is running for right now.
static IN_FLIGHT: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(|| Mutex::new(HashSet::new()));
/// Only one remote browser sign-in may own Weles at a time. OAuth refreshes
/// remain per-subscription and continue while this lock is held.
static SIGN_IN_SERIAL: LazyLock<tokio::sync::Mutex<()>> =
    LazyLock::new(|| tokio::sync::Mutex::new(()));

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
enum Swept {
    /// This grant has more than the skew window left.
    NotDue,
    /// The access token was replaced before it expired.
    Refreshed,
    /// The refresh was refused, or the refreshed grant could not be stored.
    Refused,
    /// The vault row exists but yielded no credential. Its exact account must
    /// go through Weles rather than being left unknown forever.
    AwaitingSignIn {
        subscription_id: String,
        provider: String,
    },
    /// A refresh for this subscription was already running.
    Skipped,
}

/// Walk every active subscription once, refreshing what is due.
async fn sweep(skew: Duration) {
    let automatic_sign_in = std::env::var(AUTOMATIC_SIGN_IN_ENV).is_ok_and(|value| value == "1");
    let mut visited = BTreeSet::new();
    let mut refreshed = usize::default();
    let mut refused = usize::default();
    let mut awaiting_signin = usize::default();
    let mut sign_ins_started = usize::default();
    let mut entries = Vec::new();
    for agent in broker::configured_request_sign_agents() {
        entries.extend(broker::list_subscriptions(&agent).await);
    }
    // Incomplete historical items are never handed to request dispatch. They
    // enter only this repair loop; the Weles account declaration must map back
    // to the exact subscription id before a browser opens.
    if automatic_sign_in {
        entries.extend(broker::list_recoverable_subscriptions().await);
    }
    for entry in entries {
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
        // A historical OAuth item without its exact Weles account is not fully
        // owned, even while its current access token still works. Repair that
        // identity now instead of waiting for an expiry or provider refusal:
        // the requested subscription id lets Weles accept only its declared
        // account, and a successful donation writes the durable login tag.
        if automatic_sign_in
            && entry
                .login_item
                .as_deref()
                .map(str::trim)
                .filter(|item| !item.is_empty())
                .is_none()
        {
            awaiting_signin = awaiting_signin.saturating_add(1);
            if schedule_sign_in(entry.id, entry.provider, entry.login_item) {
                sign_ins_started = sign_ins_started.saturating_add(1);
            }
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
                if automatic_sign_in && schedule_sign_in(entry.id, entry.provider, entry.login_item)
                {
                    sign_ins_started = sign_ins_started.saturating_add(1);
                }
                continue;
            }
            RefreshHint::NotDue => continue,
            RefreshHint::Read => {}
        }
        let login_item = entry.login_item;
        match refresh_one(entry.id, entry.provider, skew).await {
            Swept::Refreshed => refreshed = refreshed.saturating_add(1),
            Swept::Refused => refused = refused.saturating_add(1),
            Swept::AwaitingSignIn {
                subscription_id,
                provider,
            } => {
                awaiting_signin = awaiting_signin.saturating_add(1);
                if automatic_sign_in && schedule_sign_in(subscription_id, provider, login_item) {
                    sign_ins_started = sign_ins_started.saturating_add(1);
                }
            }
            Swept::NotDue | Swept::Skipped => {}
        }
    }
    info!(
        event = "credential_refresh_sweep_finished",
        subscriptions = visited.len(),
        refreshed,
        refused,
        awaiting_signin,
        sign_ins_started,
        automatic_sign_in,
        "finished one credential refresh sweep"
    );
}
/// Whether another automatic browser sign-in for this subscription could
/// possibly answer differently than the last one did.
///
/// Only one thing changes the outcome of a sign-in: the stored credential. The
/// ledger writes a new instant every time that changes -- a definitive refusal,
/// a rotation, a sign-in that stored something -- so a verdict no newer than
/// the newest completed sign-in proves a browser has already been driven
/// against exactly this state, and driving it again spends a real
/// single-sign-on to be told the same thing.
///
/// This gate exists because the cooldown alone was not one. On 2026-08-27
/// `brama-sub-wisent-app-codex-primary` was recorded `needs_reauthorization`
/// with the provider's own sentence, "Your session has ended. Please log in
/// again." Every sweep after that read the ledger, saw a state that is not
/// usable, and scheduled a browser sign-in; the half-hour cooldown made that a
/// rate, not a stop. By 2026-09-02 the fleet had driven more than a thousand
/// of them, three providers interleaved, one real Google login roughly every
/// ten minutes for six days. Each one resubmitted a code the account had
/// already rejected, until Google answered "Too many failed attempts" and
/// locked the authenticator method on two accounts -- so the loop did not
/// merely fail to repair the grant, it destroyed the operator's own ability to
/// repair it by hand. The ledger held the verdict that says a timer cannot fix
/// this; nothing read it.
fn verdict_outranks_last_sign_in(
    verdict_at_ms: Option<i64>,
    last_sign_in_at_ms: Option<i64>,
) -> bool {
    match (verdict_at_ms, last_sign_in_at_ms) {
        // Nothing has ever been signed in for this subscription, so the one
        // attempt an automatic repair gets has not been spent.
        (_, None) => true,
        // A verdict recorded after the last sign-in is new information: either
        // the stored credential changed, or the provider said something it had
        // not said before. Either way the answer can differ.
        (Some(verdict_at_ms), Some(signed_in_at_ms)) => verdict_at_ms > signed_in_at_ms,
        // A sign-in has completed and the ledger holds no verdict about the
        // credential at all -- the vault row yielded nothing then, and nothing
        // since has said otherwise. Another browser run reads the same row.
        (None, Some(_)) => false,
    }
}

/// Start one account sign-in without making the refresh sweep wait for a
/// browser. The claim is acquired before spawning, and the completed journal
/// record supplies a restart-safe cooldown.
///
/// Every path that can drive a browser funnels through here, so the verdict
/// gate belongs here and nowhere else.
fn schedule_sign_in(subscription_id: String, provider: String, login_item: Option<String>) -> bool {
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
        let options = super::sign_in::SignInOptions {
            provider: provider.clone(),
            login_item: login_item.clone(),
            subscription_id: Some(subscription_id.clone()),
            reason: reason.clone(),
            login_timeout_ms: sign_in_timeout_ms(),
        };
        match tokio::spawn(super::sign_in::sign_in_provider(options)).await {
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
            // `Err` means a transient preflight dependency failed before Weles
            // accepted a browser run: directory resolution or admission
            // health. Nothing account-sensitive happened, so do not write the
            // sign-in cooldown; the next sweep can use a repaired dependency.
            Ok(Err(detail)) => {
                warn!(
                    event = "credential_sign_in_preflight_failed",
                    subscription = %subscription_id,
                    provider = %provider,
                    login_item = %login_label,
                    %detail
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
                "no credential could be read for this subscription; its declared Weles account \
                 will be asked to restore the grant"
            );
            Swept::AwaitingSignIn {
                subscription_id,
                provider,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::verdict_outranks_last_sign_in;

    /// Milliseconds for 2026-08-27T00:24:00Z, the instant the codex grant was
    /// recorded `needs_reauthorization` with "Your session has ended. Please
    /// log in again."
    const REFUSAL_RECORDED_MS: i64 = 1_787_790_240_000;

    /// The defect this gate exists for: a definitively-refused credential that
    /// a browser sign-in has already been driven against must never be driven
    /// again by the sweep, however much later the sweep runs.
    #[test]
    fn a_refusal_already_signed_in_against_is_never_re_driven() {
        let first_attempt = REFUSAL_RECORDED_MS + 60_000;
        assert!(
            !verdict_outranks_last_sign_in(Some(REFUSAL_RECORDED_MS), Some(first_attempt)),
            "the sweep re-drove a browser sign-in for a credential whose recorded refusal \
             predates the sign-in already spent on it"
        );
        // Six days later, which is how long the real loop ran.
        let six_days_later = first_attempt + 6 * 24 * 60 * 60 * 1000;
        assert!(
            !verdict_outranks_last_sign_in(Some(REFUSAL_RECORDED_MS), Some(six_days_later)),
            "elapsed time is not new information about the stored credential"
        );
    }

    /// A vault row that yields no credential at all is the same wall: the
    /// ledger holds no verdict, and a sign-in has already read that row.
    #[test]
    fn a_credential_less_row_already_signed_in_against_is_never_re_driven() {
        assert!(!verdict_outranks_last_sign_in(
            None,
            Some(REFUSAL_RECORDED_MS)
        ));
    }

    /// The one automatic attempt still happens. A subscription nothing has ever
    /// signed in gets its browser run, whatever the ledger says.
    #[test]
    fn the_first_automatic_sign_in_is_allowed() {
        assert!(verdict_outranks_last_sign_in(
            Some(REFUSAL_RECORDED_MS),
            None
        ));
        assert!(verdict_outranks_last_sign_in(None, None));
    }

    /// A refusal recorded after the last sign-in is new information: the grant
    /// worked, then died, and that is exactly the case an automatic sign-in
    /// repairs without waking anybody.
    #[test]
    fn a_refusal_newer_than_the_last_sign_in_is_driven() {
        let earlier_sign_in = REFUSAL_RECORDED_MS - 60_000;
        assert!(verdict_outranks_last_sign_in(
            Some(REFUSAL_RECORDED_MS),
            Some(earlier_sign_in)
        ));
    }
}
