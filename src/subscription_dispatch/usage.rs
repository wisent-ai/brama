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
//! A fourth fact was added once it became clear that the first three cannot be
//! told apart when they are all absent: the verdict of the last proactive probe.
//! An empty set of windows means one of three unrelated things -- the provider
//! publishes none, nothing ever reached this account, or the credential is
//! refused -- and the probe verdict is what names which.
//!
//! The file is written atomically and is not a cache. It survives restarts
//! because the question it answers -- "how much of this month is gone" -- is not
//! answerable from a process that started ten seconds ago.

use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::core::failure::{self, IMPACT_CREDENTIAL_BLOCK, POINT_CREDENTIAL_BLOCK};
use crate::types::{LimitReading, ModelResponse};

const USAGE_FILE_ENV: &str = "BRAMA_SUBSCRIPTION_USAGE_FILE";
const DEFAULT_BLOCK_MS: i64 = 15 * 60 * 1_000;
const MAX_BLOCK_MS: i64 = 7 * 24 * 60 * 60 * 1_000;
// Long enough that a renewal has a chance to run, short enough that a renewed
// credential is not gated by the record of the one it replaced.
const REAUTHORIZATION_BLOCK_MS: i64 = 30 * 60 * 1_000;
// The stored reason is a sentence for an operator, not a payload.
const REASON_LIMIT: usize = 200;

static LEDGER: Mutex<Option<Ledger>> = Mutex::new(None);

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
    /// The newest proactive usage probe, when one has run.
    ///
    /// This is what separates the three reasons a subscription reports no plan
    /// window. Without it, a provider that publishes nothing, an account no
    /// traffic ever reached, and a credential the provider rejects are one
    /// indistinguishable blank.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub probe: Option<Probe>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Measured {
    #[serde(default)]
    pub requests: u64,
    #[serde(default)]
    pub failures: u64,
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_used_ms: Option<i64>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Block {
    pub blocked_until_ms: i64,
    pub reason: String,
    pub recorded_at_ms: i64,
    /// The same refusal in the fleet's envelope, as JSON. An extra key rather
    /// than a replacement: the three above are what other tooling reads out of
    /// this file, and a reader that does not know this one ignores it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub envelope: Option<String>,
}

/// The outcome of one proactive usage probe.
///
/// `ok` is about the provider call, not about the plan: a provider that answered
/// and publishes no window at all is a successful probe with no readings, and
/// that is precisely the state no reader could name before.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Probe {
    pub attempted_at_ms: i64,
    pub ok: bool,
    /// The provider's own sentence when it refused, trimmed like every other
    /// stored reason here. Absent on success, where there is nothing to explain.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct Ledger {
    #[serde(default)]
    subscriptions: BTreeMap<String, SubscriptionUsage>,
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or_default()
}

fn usage_path() -> Option<PathBuf> {
    if let Ok(configured) = std::env::var(USAGE_FILE_ENV) {
        let trimmed = configured.trim();
        if !trimmed.is_empty() {
            return Some(PathBuf::from(trimmed));
        }
    }
    let home = std::env::var("HOME").ok()?;
    Some(
        PathBuf::from(home)
            .join(".config")
            .join("brama")
            .join("subscription-usage.json"),
    )
}

/// Read the ledger, tolerating anything the file might be.
///
/// A ledger that will not parse yields an empty one rather than an error: this
/// file is a record, not a configuration, and a gateway that refuses to start
/// because a usage number is malformed trades every request for one statistic.
fn load() -> Ledger {
    let Some(path) = usage_path() else {
        return Ledger::default();
    };
    let Ok(text) = fs::read_to_string(&path) else {
        return Ledger::default();
    };
    let mut ledger: Ledger = serde_json::from_str(&text).unwrap_or_default();
    backfill_reading_times(&mut ledger, &path);
    ledger
}

/// Give readings written before `recorded_at_ms` existed the ledger file's own
/// timestamp.
///
/// Those readings were observed at some point before the file was last written,
/// so the file's modification time is the tightest upper bound available and is
/// closer to the truth than the epoch. It is an upper bound, not a measurement,
/// which is why it is only ever applied to a reading that carries no instant of
/// its own.
fn backfill_reading_times(ledger: &mut Ledger, path: &std::path::Path) {
    let fallback = fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
        .and_then(|elapsed| i64::try_from(elapsed.as_millis()).ok())
        .unwrap_or_default();
    if fallback == 0 {
        return;
    }
    for usage in ledger.subscriptions.values_mut() {
        for reading in usage.limits.values_mut() {
            if reading.recorded_at_ms == 0 {
                reading.recorded_at_ms = fallback;
            }
        }
    }
}

/// Write through a private temporary file in the same directory and rename, so
/// a reader never sees a half-written ledger and a crash never truncates one.
fn persist(ledger: &Ledger) {
    let Some(path) = usage_path() else {
        return;
    };
    let Some(parent) = path.parent() else {
        return;
    };
    if fs::create_dir_all(parent).is_err() {
        return;
    }
    let Ok(bytes) = serde_json::to_vec_pretty(ledger) else {
        return;
    };
    let staging = parent.join(format!(".subscription-usage.{}.tmp", std::process::id()));
    let written = (|| -> std::io::Result<()> {
        let mut options = OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&staging)?;
        file.write_all(&bytes)?;
        file.write_all(b"\n")?;
        file.sync_all()
    })();
    if written.is_err() || fs::rename(&staging, &path).is_err() {
        let _ = fs::remove_file(&staging);
    }
}

fn with_ledger<T>(apply: impl FnOnce(&mut Ledger) -> T) -> T {
    let mut guard = match LEDGER.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    if guard.is_none() {
        *guard = Some(load());
    }
    let ledger = guard.as_mut().unwrap_or_else(|| unreachable!());
    let outcome = apply(ledger);
    persist(ledger);
    outcome
}

/// Record one provider call against the subscription that paid for it.
pub fn record_call(subscription_id: &str, provider: &str, response: &ModelResponse) {
    let now = now_ms();
    with_ledger(|ledger| {
        let entry = ledger
            .subscriptions
            .entry(subscription_id.to_string())
            .or_default();
        entry.provider = provider.to_string();
        entry.updated_at_ms = Some(now);
        entry.measured.since_ms.get_or_insert(now);
        entry.measured.last_used_ms = Some(now);
        entry.measured.requests = entry.measured.requests.saturating_add(1);
        if !response.success {
            entry.measured.failures = entry.measured.failures.saturating_add(1);
        }
        entry.measured.input_tokens = entry
            .measured
            .input_tokens
            .saturating_add(u64::from(response.input_tokens));
        entry.measured.output_tokens = entry
            .measured
            .output_tokens
            .saturating_add(u64::from(response.output_tokens));
        for reading in &response.limits {
            entry
                .limits
                .insert(reading.limit_id.clone(), reading.clone());
        }
        // A window that is no longer exhausted clears the block without waiting
        // for its deadline: the provider just answered, which is better evidence
        // than the estimate that produced the block.
        if response.success {
            entry.block = None;
        }
    });
}

/// Mark a subscription unusable until an instant.
///
/// The provider's own reset time is preferred over any local guess; the default
/// is used only when the answer carried no reset at all, and it is bounded so a
/// malformed header cannot retire a credential for a year.
pub fn record_block(subscription_id: &str, provider: &str, reason: &str, response: &ModelResponse) {
    let now = now_ms();
    let from_provider = response
        .limits
        .iter()
        .filter(|reading| reading.used_fraction >= 1.0 || response.limits.len() == 1)
        .filter_map(|reading| reading.resets_at_ms)
        .filter(|resets| *resets > now)
        .min();
    let until = from_provider
        .unwrap_or(now.saturating_add(DEFAULT_BLOCK_MS))
        .min(now.saturating_add(MAX_BLOCK_MS));
    // A block is a rate limit the provider stated. The sentence it stated is
    // kept in `reason` for the tooling that already reads it and in the
    // envelope for the operator who needs to know it is transient.
    let blocked = failure::envelope(
        POINT_CREDENTIAL_BLOCK,
        failure::code_for("provider_rate_limited"),
        IMPACT_CREDENTIAL_BLOCK,
        reason,
    )
    .with_context("subscription", subscription_id)
    .with_context("provider", provider);
    with_ledger(|ledger| {
        let entry = ledger
            .subscriptions
            .entry(subscription_id.to_string())
            .or_default();
        entry.provider = provider.to_string();
        entry.updated_at_ms = Some(now);
        entry.block = Some(Block {
            blocked_until_ms: until,
            reason: reason.chars().take(REASON_LIMIT).collect(),
            recorded_at_ms: now,
            envelope: Some(blocked.to_json()),
        });
    });
}

/// Record that a credential can no longer be renewed on its own, because the
/// rotated grant was lost.
///
/// A provider that rotates refresh tokens invalidates the previous one the
/// moment it issues a new one. If that new grant is not written back, the copy
/// in the vault is already dead and every later use fails with `invalid_grant`
/// no matter how healthy the account is. This is not a rate limit and it is not
/// transient: it needs a re-authorization, and the window is short so that a
/// successful renewal is not gated by a stale record.
pub fn record_reauthorization_needed(subscription_id: &str, provider: &str, reason: &str) {
    let now = now_ms();
    let recorded = failure::envelope(
        POINT_CREDENTIAL_BLOCK,
        failure::code_for("credential_unauthorized"),
        IMPACT_CREDENTIAL_BLOCK,
        reason,
    )
    .with_context("subscription", subscription_id)
    .with_context("provider", provider);
    with_ledger(|ledger| {
        let entry = ledger
            .subscriptions
            .entry(subscription_id.to_string())
            .or_default();
        entry.provider = provider.to_string();
        entry.updated_at_ms = Some(now);
        entry.block = Some(Block {
            blocked_until_ms: now.saturating_add(REAUTHORIZATION_BLOCK_MS),
            reason: reason.chars().take(REASON_LIMIT).collect(),
            recorded_at_ms: now,
            envelope: Some(recorded.to_json()),
        });
    });
}

/// Record what a proactive usage probe learned about one subscription.
///
/// The probe exists because a plan window only ever appeared for the one account
/// that happened to serve a request, which left every other row indistinguishable
/// from a provider that publishes nothing. Storing the attempt -- including a
/// refused one, with the provider's own sentence -- is what makes the difference
/// readable. Any readings the probe's answer carried are recorded by
/// [`record_call`] on the same path real traffic takes; this only adds the
/// verdict.
pub fn record_probe(subscription_id: &str, provider: &str, ok: bool, detail: Option<&str>) {
    let now = now_ms();
    let detail = detail
        .map(str::trim)
        .filter(|detail| !detail.is_empty())
        .map(|detail| detail.chars().take(REASON_LIMIT).collect::<String>());
    with_ledger(|ledger| {
        let entry = ledger
            .subscriptions
            .entry(subscription_id.to_string())
            .or_default();
        entry.provider = provider.to_string();
        entry.updated_at_ms = Some(now);
        entry.probe = Some(Probe {
            attempted_at_ms: now,
            ok,
            detail,
        });
    });
}

/// Whether this subscription is inside a recorded block right now.
pub fn is_blocked(subscription_id: &str) -> bool {
    let now = now_ms();
    with_ledger(|ledger| {
        ledger
            .subscriptions
            .get(subscription_id)
            .and_then(|entry| entry.block.as_ref())
            .is_some_and(|block| block.blocked_until_ms > now)
    })
}

/// The recorded state of one subscription, if anything was ever recorded.
pub fn usage_for(subscription_id: &str) -> Option<SubscriptionUsage> {
    with_ledger(|ledger| ledger.subscriptions.get(subscription_id).cloned())
}
