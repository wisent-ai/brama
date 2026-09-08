//! How a reading already being taken is shared, and how long it is allowed to
//! take.
//!
//! One subscription's report is one question to one provider, so a second
//! caller asking while the first is still waiting joins that read instead of
//! issuing another one -- the registry here is what makes "at most once per
//! window" true even when the console, an agent and the sweep all ask in the
//! same second. It is separate from the read because everything in it is about
//! the shape of the wait rather than about usage: the operation deadline, the
//! slightly longer deadline joiners get so they observe a result rather than
//! their own timeout, and the guarantee that whoever waited is told why if no
//! result was ever published.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use tokio::sync::watch;
use wisent_errors::{Code, Failure};

use super::refresh_once;
use super::refusal::{record_refusal, scoped_failure, POINT_USAGE_LEDGER_PERSIST};

/// Whole-operation bound: credential redemption, at most one OAuth refresh, and
/// at most two provider usage GETs must all finish inside it.
const REFRESH_TIMEOUT_SECS: u64 = 60;
/// Joiners get a small margin beyond the worker deadline to observe its result.
const REFRESH_JOIN_TIMEOUT_SECS: u64 = REFRESH_TIMEOUT_SECS + 5;
const POINT_PLAN_USAGE_REFRESH: &str = "brama.subscription-usage.refresh";

pub(super) type RefreshResult = Result<(), Failure>;
pub(super) type RefreshReceiver = watch::Receiver<Option<RefreshResult>>;

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

fn finished(receiver: &RefreshReceiver) -> Option<RefreshResult> {
    receiver.borrow().as_ref().cloned()
}

pub(super) async fn join_refresh(
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

pub(super) fn shared_refresh(subscription_id: &str, provider: &str) -> RefreshReceiver {
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
