//! What each subscription has spent, what its plan says is left, and until when
//! it is unusable.
//!
//! Three facts live here and they are deliberately not merged. What Brama spent
//! is Brama's own measurement and is always available. What fraction of a plan
//! window is used, and when that window resets, is the provider's statement
//! about its own quota, read from the headers it already returns; Brama never
//! computes it, because a locally derived percentage of a limit Brama does not
//! own would be a second account that silently disagrees with the vendor's. A
//! block is the third: a rate-limited answer is turned into an instant before
//! which this credential must not be tried again, so the dispatcher stops
//! guessing from error strings on every call.
//!
//! A fourth fact is the verdict of the newest provider-only usage check. An
//! empty set of windows can mean that the provider explicitly published none,
//! nothing ever reached this account, or the credential was refused; the usage
//! check names which without being overwritten by a completion probe.
//!
//! The completion probe remains a separate fifth fact because it answers
//! whether inference itself worked and may run later than a failed free usage
//! read. A sixth records whether the provider still accepts the credential at
//! all. A disowned credential is not a plan window, block, or probe verdict,
//! and while it had nowhere to be recorded it rendered as an account that
//! simply had a quiet week. The credential state says whether a sign-in is
//! overdue.
//!
//! The file is written atomically and is not a cache. It survives restarts
//! because the question it answers -- "how much of this month is gone" -- is not
//! answerable from a process that started ten seconds ago.
//!
//! This file is the ledger itself: one subscription's whole record, the single
//! copy this process holds, and the lock every reader and writer goes through.
//! Where that copy is stored and how a new one replaces it lives in
//! `ledger_file`. What a call cost the account, and until when a rate limit
//! forbids the next one, lives in `spend`. The provider's own windows and how
//! current they are live in `plan_window`. The two checks Brama runs on purpose
//! live in `check`, and where a grant stands with its provider lives in
//! `credential`.

mod check;
mod credential;
mod ledger_file;
mod plan_window;
mod spend;

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::types::LimitReading;

use self::ledger_file::{load, persist};

// Every name the rest of the crate already reaches through this module,
// re-exported one by one so no caller path changes and nothing that was private
// to the ledger travels out with them.
pub use self::check::{
    plan_usage_check, record_plan_usage, record_plan_usage_failure, record_plan_usage_unpublished,
    record_probe, CheckSource, Probe,
};
pub use self::credential::{
    awaiting_sign_in_cause, credential_recorded_at_ms, credential_refresh_hint,
    needs_reauthorization, record_credential_active, record_credential_disabled,
    record_credential_signed_in, record_reauthorization_needed, Credential, CredentialState,
    RefreshHint,
};
pub use self::plan_window::{
    jittered_plan_usage_ttl_ms, next_reset_ms, plan_usage_due, plan_usage_retention_ms,
    plan_usage_ttl_ms, plan_windows, used_fraction, PlanWindows, UsageSource,
};
pub use self::spend::{is_blocked, record_block, record_call, record_call_from, Block, Measured};

// The stored reason is a sentence for an operator, not a payload.
const REASON_LIMIT: usize = 200;

static LEDGER: Mutex<Option<LedgerState>> = Mutex::new(None);

/// Everything recorded about one subscription.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct SubscriptionUsage {
    pub provider: String,
    #[serde(default)]
    pub measured: Measured,
    /// Latest reading per limit id, newest wins. A window the provider stopped
    /// reporting keeps its last reading rather than vanishing from the view.
    #[serde(default)]
    pub limits: BTreeMap<String, LimitReading>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub block: Option<Block>,
    /// When anything in this record last changed.
    ///
    /// A reader cannot otherwise tell a subscription nobody has touched for a
    /// week from one that answered a second ago, because both render as the same
    /// set of numbers. Absent only for records written before this field
    /// existed; every mutation below sets it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at_ms: Option<i64>,
    /// The newest explicitly requested completion probe, when one has run.
    ///
    /// This answers whether inference worked. It is not evidence that the free
    /// usage endpoint was readable; that verdict lives in `usage_check`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub probe: Option<Probe>,
    /// The newest free provider usage-report attempt.
    ///
    /// Kept apart from `probe`, which records a completion check: ordinary
    /// traffic or a later completion must never erase the reason the usage
    /// report itself could not be read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage_check: Option<Probe>,
    /// Full failure from the newest refused free usage attempt.
    ///
    /// This is ledger-internal evidence for read-only reports, not a duplicate
    /// wire field. `Probe` intentionally remains the small public check shape.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage_failure: Option<serde_json::Value>,
    /// Where the newest plan window came from.
    ///
    /// Three sources state the same kind of fact with different standing: the
    /// provider's own usage report, the headers of real traffic, and an
    /// operator's on-demand probe. A reader that cannot tell them apart cannot
    /// say whether a window is the provider's current statement or a side
    /// effect of somebody's request an hour ago.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage_source: Option<UsageSource>,
    /// When this subscription's usage report was last checked, whatever the
    /// outcome was.
    ///
    /// Deliberately not one of the readings' own instants: a check that found a
    /// refusal, or a provider that publishes no report at all, moves this and
    /// leaves the readings alone. It is the instant the cache window below is
    /// measured against.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_usage_checked_at_ms: Option<i64>,
    /// Where this subscription's grant stands with its provider.
    ///
    /// Deliberately separate from `block`: a block is a quota the provider
    /// hands back on its own schedule, while this is whether the credential is
    /// still accepted at all. A refused grant recorded only as a block reads as
    /// a rate limit that never clears.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential: Option<Credential>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct Ledger {
    #[serde(default)]
    subscriptions: BTreeMap<String, SubscriptionUsage>,
}
#[derive(Debug)]
struct LedgerState {
    ledger: Ledger,
    storage_error: Option<String>,
    /// A ledger that existed but could not be read or decoded is never replaced
    /// by the empty in-memory substitute. New observations remain visible in
    /// this process, but only repairing the persisted history can make writes
    /// safe.
    persist_blocked: bool,
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or_default()
}

fn ledger_state(guard: &mut Option<LedgerState>) -> &mut LedgerState {
    if guard.is_none() {
        *guard = Some(load());
    }
    guard.as_mut().unwrap_or_else(|| unreachable!())
}

/// Mutate the process ledger and attempt to commit the resulting whole state.
///
/// The mutation is retained even when the write fails, so a report in this
/// process still shows the newest observation alongside [`storage_error`].
fn write_ledger<T>(apply: impl FnOnce(&mut Ledger) -> T) -> (T, Result<(), String>) {
    let mut guard = match LEDGER.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    let state = ledger_state(&mut guard);
    let outcome = apply(&mut state.ledger);
    let stored = if state.persist_blocked {
        Err(state.storage_error.clone().unwrap_or_else(|| {
            "subscription usage ledger persistence is blocked by an earlier load failure"
                .to_string()
        }))
    } else {
        persist(&state.ledger)
    };
    match &stored {
        Ok(()) => state.storage_error = None,
        Err(error) => state.storage_error = Some(error.clone()),
    }
    (outcome, stored)
}

fn with_ledger<T>(apply: impl FnOnce(&mut Ledger) -> T) -> T {
    write_ledger(apply).0
}

/// Read the current process ledger without rewriting it.
fn read_ledger<T>(apply: impl FnOnce(&Ledger) -> T) -> T {
    let mut guard = match LEDGER.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    apply(&ledger_state(&mut guard).ledger)
}

/// Mutate only the current process view after a failed commit.
fn with_ledger_memory<T>(apply: impl FnOnce(&mut Ledger) -> T) -> T {
    let mut guard = match LEDGER.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    apply(&mut ledger_state(&mut guard).ledger)
}

/// The actual ledger load or persistence failure still affecting this process.
pub fn storage_error() -> Option<String> {
    let mut guard = match LEDGER.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    ledger_state(&mut guard).storage_error.clone()
}

/// The recorded state of one subscription, if anything was ever recorded.
pub fn usage_for(subscription_id: &str) -> Option<SubscriptionUsage> {
    read_ledger(|ledger| ledger.subscriptions.get(subscription_id).cloned())
}

/// Every subscription in the current process ledger, without writing it back.
///
/// This uses the same in-memory state as [`usage_for`], including observations
/// whose atomic write failed. [`storage_error`] tells the reporter that the
/// persisted copy is behind.
pub fn recorded_subscriptions() -> BTreeMap<String, SubscriptionUsage> {
    read_ledger(|ledger| ledger.subscriptions.clone())
}
