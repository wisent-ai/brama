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
//! window, because provider usage endpoints may rate-limit per source address
//! and several accounts on one host share that address. Each
//! subscription's window is spread by up to a quarter either way, derived from
//! its own id, so those accounts never come due in the same second. And a
//! failed read never replaces a good reading: the last one is kept and marked
//! stale immediately, so a row says both what it last knew and why that reading
//! is no longer current instead of blanking over one bad minute upstream.
//!
//! A provider credential with no supported free usage-report endpoint is
//! recorded that way. The vendor may expose separately privileged billing APIs;
//! Brama states only what this credential can read without inference.

use std::collections::{BTreeSet, HashMap};
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use tokio::sync::watch;
use tracing::{info, warn};
use wisent_errors::{Code, Failure};

use crate::core::failure::{self, POINT_CREDENTIAL_REDEEM, POINT_PROVIDER_CALL};
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
/// Whole-operation bound: credential redemption, at most one OAuth refresh, and
/// at most two provider usage GETs must all finish inside it.
const REFRESH_TIMEOUT_SECS: u64 = 60;
/// Joiners get a small margin beyond the worker deadline to observe its result.
const REFRESH_JOIN_TIMEOUT_SECS: u64 = REFRESH_TIMEOUT_SECS + 5;
const POINT_PLAN_USAGE_REFRESH: &str = "brama.subscription-usage.refresh";
const POINT_USAGE_LEDGER_PERSIST: &str = "brama.subscription-usage.ledger-persist";
const IMPACT_PLAN_USAGE: &str = "this subscription's current usage report";
/// What is recorded when Brama supports no free usage-report endpoint for this
/// provider credential.
///
/// This deliberately makes no claim about separately privileged organization
/// billing APIs the vendor may offer.
const UNPUBLISHED_DETAIL: &str = "Brama has no supported free usage-report endpoint for this \
     provider credential; plan windows are recorded only from real traffic";

type RefreshResult = Result<(), Failure>;
type RefreshReceiver = watch::Receiver<Option<RefreshResult>>;

/// The shared result of each in-progress subscription/provider read.
///
/// A waiter joins the same bounded operation rather than silently skipping it
/// or issuing a second provider request.
static REFRESHING: LazyLock<Mutex<HashMap<String, RefreshReceiver>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
struct RefreshRegistration {
    key: String,
}

impl Drop for RefreshRegistration {
    fn drop(&mut self) {
        let mut guard = match REFRESHING.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        guard.remove(&self.key);
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

fn scoped_failure(
    point: &str,
    code: Code,
    subscription_id: &str,
    provider: &str,
    detail: impl Into<String>,
) -> Failure {
    failure::envelope(point, code, IMPACT_PLAN_USAGE, detail)
        .with_context("subscription", subscription_id)
        .with_context("provider", provider)
}

fn provider_status(detail: &str) -> Option<u16> {
    let (_, status) = detail.split_once(" returned HTTP ")?;
    let digits = status
        .chars()
        .take_while(char::is_ascii_digit)
        .collect::<String>();
    digits.parse().ok()
}

fn provider_failure(subscription_id: &str, provider: &str, detail: String) -> Failure {
    let code = provider_status(&detail)
        .filter(|status| !(200..300).contains(status))
        .map(Code::from_upstream_status)
        .unwrap_or_else(|| failure::code_for_message(&detail, "provider_failure"));
    scoped_failure(POINT_PROVIDER_CALL, code, subscription_id, provider, detail)
}

fn invalid_credential_failure(subscription_id: &str, provider: &str, item: &str) -> Failure {
    scoped_failure(
        POINT_CREDENTIAL_REDEEM,
        Code::Config,
        subscription_id,
        provider,
        format!("subscription credential `{item}` is not valid UTF-8"),
    )
}

fn record_refusal(subscription_id: &str, provider: &str, refused: Failure) -> Failure {
    match usage::record_plan_usage_failure(subscription_id, provider, &refused) {
        Ok(()) => refused,
        Err(storage) => storage,
    }
}

fn finished(receiver: &RefreshReceiver) -> Option<RefreshResult> {
    receiver.borrow().as_ref().cloned()
}

async fn join_refresh(
    mut receiver: RefreshReceiver,
    subscription_id: &str,
    provider: &str,
) -> RefreshResult {
    if let Some(result) = finished(&receiver) {
        return result;
    }
    match tokio::time::timeout(
        Duration::from_secs(REFRESH_JOIN_TIMEOUT_SECS),
        receiver.changed(),
    )
    .await
    {
        Ok(Ok(())) => finished(&receiver).unwrap_or_else(|| {
            Err(scoped_failure(
                POINT_PLAN_USAGE_REFRESH,
                Code::Unknown,
                subscription_id,
                provider,
                "the shared usage refresh ended without publishing a result",
            ))
        }),
        Ok(Err(_)) => Err(scoped_failure(
            POINT_PLAN_USAGE_REFRESH,
            Code::Unknown,
            subscription_id,
            provider,
            "the shared usage refresh channel closed without publishing a result",
        )),
        Err(_) => Err(scoped_failure(
            POINT_PLAN_USAGE_REFRESH,
            Code::Timeout,
            subscription_id,
            provider,
            format!(
                "timed out after {REFRESH_JOIN_TIMEOUT_SECS} seconds waiting for the in-progress \
                 usage refresh"
            ),
        )),
    }
}

fn shared_refresh(subscription_id: &str, provider: &str) -> RefreshReceiver {
    let key = format!("{provider}\0{subscription_id}");
    let mut guard = match REFRESHING.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    if let Some(receiver) = guard.get(&key) {
        return receiver.clone();
    }
    let (sender, receiver) = watch::channel(None);
    guard.insert(key.clone(), receiver.clone());
    let registration = RefreshRegistration { key };
    let subscription_id = subscription_id.to_string();
    let provider = provider.to_string();
    tokio::spawn(async move {
        let _registration = registration;
        let result = run_bounded_refresh(&subscription_id, &provider).await;
        sender.send_replace(Some(result));
    });
    receiver
}

async fn run_bounded_refresh(subscription_id: &str, provider: &str) -> RefreshResult {
    let owned_id = subscription_id.to_string();
    let owned_provider = provider.to_string();
    let mut worker = tokio::spawn(async move { refresh_once(&owned_id, &owned_provider).await });
    let result =
        match tokio::time::timeout(Duration::from_secs(REFRESH_TIMEOUT_SECS), &mut worker).await {
            Ok(Ok(result)) => result,
            Ok(Err(error)) => Err(scoped_failure(
                POINT_PLAN_USAGE_REFRESH,
                Code::Unknown,
                subscription_id,
                provider,
                format!("usage refresh task failed to join: {error}"),
            )),
            Err(_) => {
                worker.abort();
                Err(scoped_failure(
                    POINT_PLAN_USAGE_REFRESH,
                    Code::Timeout,
                    subscription_id,
                    provider,
                    format!(
                    "usage refresh exceeded its {REFRESH_TIMEOUT_SECS}-second operation deadline"
                ),
                ))
            }
        };
    match result {
        Err(refused) if refused.failure_point != POINT_USAGE_LEDGER_PERSIST => {
            Err(record_refusal(subscription_id, provider, refused))
        }
        result => result,
    }
}

fn is_provider_authentication(detail: &str) -> bool {
    detail
        .split_once(':')
        .is_some_and(|(kind, _)| kind.trim() == "provider_authentication")
}

async fn refresh_once(subscription_id: &str, provider: &str) -> RefreshResult {
    if !provider_registry::publishes_plan_usage(provider) {
        usage::record_plan_usage_unpublished(subscription_id, provider, UNPUBLISHED_DETAIL)?;
        info!(
            event = "plan_usage_unpublished",
            subscription = %subscription_id,
            provider = %provider,
            "Brama has no supported free usage-report endpoint for this provider credential"
        );
        return Ok(());
    }

    let item = broker::subscription_resource(provider, subscription_id);
    let credential = broker::subscription_credential(subscription_id, provider).await?;
    let token = credential
        .expose_utf8()
        .map_err(|_| invalid_credential_failure(subscription_id, provider, &item))?;
    let first = provider_registry::read_plan_usage(provider, &item, token).await;
    let outcome = match first {
        PlanUsage::Refused(detail)
            if is_provider_authentication(&detail) && broker::supports_oauth_refresh(provider) =>
        {
            let rejected = provider_failure(subscription_id, provider, detail);
            drop(credential);
            let fresh = broker::refresh_subscription_credential(subscription_id, provider)
                .await
                .map_err(|refused| refused.caused_by(rejected.clone()))?;
            let fresh_token = fresh.expose_utf8().map_err(|_| {
                invalid_credential_failure(subscription_id, provider, &item).caused_by(rejected)
            })?;
            provider_registry::read_plan_usage(provider, &item, fresh_token).await
        }
        outcome => outcome,
    };

    match outcome {
        PlanUsage::Report(readings) => {
            let windows = readings.len();
            usage::record_plan_usage(subscription_id, provider, &readings)?;
            info!(
                event = "plan_usage_recorded",
                subscription = %subscription_id,
                provider = %provider,
                windows,
                "read one provider usage report; no quota was spent"
            );
            Ok(())
        }
        PlanUsage::Unpublished => {
            usage::record_plan_usage_unpublished(subscription_id, provider, UNPUBLISHED_DETAIL)
        }
        PlanUsage::Refused(detail) => {
            warn!(
                event = "plan_usage_refused",
                subscription = %subscription_id,
                provider = %provider,
                detail = %detail,
                "the provider would not state this subscription's usage; the last good reading \
                 is retained and stale"
            );
            Err(provider_failure(subscription_id, provider, detail))
        }
    }
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

/// Read one subscription's provider-only usage report and record the outcome.
///
/// Concurrent callers join the same bounded operation and receive its same
/// success or detailed failure.
pub async fn refresh(subscription_id: &str, provider: &str) -> Result<(), Failure> {
    join_refresh(
        shared_refresh(subscription_id, provider),
        subscription_id,
        provider,
    )
    .await
}
