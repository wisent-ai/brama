//! Read what each subscription's plan has left from the provider's own usage
//! report, and keep that reading current without spending any of the plan.
//!
//! A provider that rations a subscription publishes a report of how much of the
//! ration is gone. Reading it costs a request and no quota: no completion, no
//! output tokens, nothing that appears in the account's own usage. That is the
//! whole difference from what this gateway did before, which was to buy the
//! answer with one small completion per subscription every quarter hour --
//! spending the thing it was trying to measure, on a timer, forever.
//!
//! Three rules hold here, and each of them is about the report being cheap but
//! not free of consequence. It is read at most once per subscription per cache
//! window, because both providers that publish one rate-limit it per source
//! address and seven accounts on one host share that address. Each
//! subscription's window is spread by up to a quarter either way, derived from
//! its own id, so those seven accounts never come due in the same second. And a
//! failed read never replaces a good reading: the last one is kept, marked stale
//! once it ages past the window, and a row keeps saying what it last knew
//! instead of blanking over one bad minute upstream.
//!
//! A provider that publishes nothing is recorded as publishing nothing. That is
//! a fact about the vendor, and without it an operator staring at an empty plan
//! cannot tell a provider that states nothing from a gateway that never asked.

use std::collections::{BTreeSet, HashSet};
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use tracing::{info, warn};

use crate::gateway::broker;
use crate::providers::adapter::{self as provider_registry, PlanUsage};
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
/// What is recorded for a provider that publishes no usage report at all.
///
/// A sentence rather than a flag, because it is shown to a person who is looking
/// at a blank plan and needs to know that the blank is the answer.
const UNPUBLISHED_DETAIL: &str = "this provider publishes no usage report, so plan windows are \
     recorded only from the answers it gives to real traffic";

/// The subscriptions being read right now.
///
/// A console poll, a sweep tick and an operator can all ask for the same
/// subscription at once, and three simultaneous reads of one report would be
/// exactly the burst the spread cache window exists to prevent.
static REFRESHING: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(|| Mutex::new(HashSet::new()));

/// One in-progress read, released when this guard is dropped -- including when
/// the read panics, which is what keeps a crash from wedging a subscription out
/// of every later sweep.
struct SingleFlight {
    subscription_id: String,
}

impl SingleFlight {
    fn claim(subscription_id: &str) -> Option<Self> {
        let mut guard = match REFRESHING.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        guard.insert(subscription_id.to_string()).then(|| Self {
            subscription_id: subscription_id.to_string(),
        })
    }
}

impl Drop for SingleFlight {
    fn drop(&mut self) {
        let mut guard = match REFRESHING.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        guard.remove(&self.subscription_id);
    }
}

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

/// Read one subscription's report, surviving anything it does.
///
/// The work runs in its own task so that a panic below -- in a provider client, a
/// credential parser, or a report reader -- is a logged join error against one
/// subscription instead of the silent death of the whole timer.
async fn refresh_isolated(subscription_id: String, provider: String) {
    let logged_id = subscription_id.clone();
    let logged_provider = provider.clone();
    if let Err(error) = tokio::spawn(async move { refresh(&subscription_id, &provider).await }).await
    {
        warn!(
            event = "plan_usage_panicked",
            subscription = %logged_id,
            provider = %logged_provider,
            %error,
            "reading one provider usage report died; the remaining subscriptions are unaffected"
        );
    }
}

/// Read one subscription's plan usage from its provider and record what came
/// back, whichever of the three answers it was.
pub async fn refresh(subscription_id: &str, provider: &str) {
    if !provider_registry::publishes_plan_usage(provider) {
        // Recorded rather than skipped, and recorded no more often than a report
        // would have been read, so the ledger states the fact without being
        // rewritten every minute over a provider that will never change its mind.
        usage::record_plan_usage_unpublished(subscription_id, provider, UNPUBLISHED_DETAIL);
        info!(
            event = "plan_usage_unpublished",
            subscription = %subscription_id,
            provider = %provider,
            "this provider publishes no usage report; recorded as such"
        );
        return;
    }
    let Some(_flight) = SingleFlight::claim(subscription_id) else {
        return;
    };
    let Some(token) = broker::subscription_credential(subscription_id, provider).await else {
        // The same sentence the request path has always recorded for this, so a
        // reader that distinguishes a local redemption failure from a provider's
        // refusal keeps distinguishing them.
        usage::record_plan_usage_refused(subscription_id, provider, "credential unavailable");
        warn!(
            event = "plan_usage_credential_unavailable",
            subscription = %subscription_id,
            provider = %provider,
            "the subscription credential could not be redeemed, so its plan could not be read"
        );
        return;
    };
    let token = match token.expose_utf8() {
        Ok(token) => token,
        Err(_) => {
            usage::record_plan_usage_refused(
                subscription_id,
                provider,
                "credential is not valid UTF-8",
            );
            return;
        }
    };
    let item = broker::subscription_resource(provider, subscription_id);
    match provider_registry::read_plan_usage(provider, &item, token).await {
        PlanUsage::Report(readings) => {
            let windows = readings.len();
            usage::record_plan_usage(subscription_id, provider, &readings);
            info!(
                event = "plan_usage_recorded",
                subscription = %subscription_id,
                provider = %provider,
                windows,
                "read one provider usage report; no quota was spent"
            );
        }
        // Unreachable through the check above, and still handled: the answer
        // belongs to the provider registry, and a table it grows later must not
        // arrive here as a refusal.
        PlanUsage::Unpublished => {
            usage::record_plan_usage_unpublished(subscription_id, provider, UNPUBLISHED_DETAIL);
        }
        PlanUsage::Refused(detail) => {
            usage::record_plan_usage_refused(subscription_id, provider, &detail);
            warn!(
                event = "plan_usage_refused_recorded",
                subscription = %subscription_id,
                provider = %provider,
                detail = %detail,
                "the provider would not state this subscription's usage; the last good reading \
                 is kept and marked stale"
            );
        }
    }
}
