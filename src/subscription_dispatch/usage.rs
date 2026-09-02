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
//! A fifth was added for the same reason the fourth was: a credential the
//! provider has disowned is not a plan window, not a block, and not a probe
//! verdict, and while it had nowhere to be recorded it rendered as an account
//! that simply had a quiet week. Four credentials sat refused for five days
//! looking exactly like that. The credential state says whether the grant
//! itself is still something the provider accepts, in the provider's own
//! words, so a reader can tell "nothing happened here" from "a sign-in is
//! overdue".
//!
//! The file is written atomically and is not a cache. It survives restarts
//! because the question it answers -- "how much of this month is gone" -- is not
//! answerable from a process that started ten seconds ago.

use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

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
/// How long one subscription's plan reading is treated as current.
///
/// Five minutes is short enough that a five-hour window never ages past its own
/// reset unnoticed, and long enough that seven accounts polling their providers
/// cost seven requests per five minutes rather than one per console refresh.
/// Both providers that publish a usage report rate-limit it per source address.
const PLAN_USAGE_TTL_ENV: &str = "BRAMA_PLAN_USAGE_TTL_SECS";
const DEFAULT_PLAN_USAGE_TTL_SECS: i64 = 5 * 60;
/// How long a last good reading is still worth serving after it went stale.
///
/// A day. An upstream that fails for an afternoon must not blank a row that was
/// correct that morning: a reading with its own instant beside it is information,
/// and an empty plan is not. Past a day the reading stops being served, because
/// a percentage of a window that has since reset several times is no longer a
/// statement about anything.
const PLAN_USAGE_RETENTION_ENV: &str = "BRAMA_PLAN_USAGE_RETENTION_SECS";
const DEFAULT_PLAN_USAGE_RETENTION_SECS: i64 = 24 * 60 * 60;
/// The spread applied to each subscription's own refresh window, either side of
/// the nominal one.
///
/// Seven accounts that all became due in the same second would fan out into one
/// burst against one provider from one address, which is what a provider's
/// per-address rate limit exists to refuse. The spread is derived from the
/// subscription id rather than drawn at random, so a row's window is the same
/// on every read and the accounts stay decorrelated across restarts.
const PLAN_USAGE_TTL_JITTER_PERCENT: i64 = 25;
const PERCENT: i64 = 100;

const MS_PER_SECOND: i64 = 1_000;

/// A duration in seconds an operator may override, floored at one second so a
/// zero cannot turn a cache window into a busy loop.
fn configured_seconds(name: &str, default_seconds: i64) -> i64 {
    std::env::var(name)
        .ok()
        .and_then(|value| value.trim().parse::<i64>().ok())
        .filter(|seconds| *seconds > 0)
        .unwrap_or(default_seconds)
}

/// The freshness window a reading is judged against, in milliseconds.
pub fn plan_usage_ttl_ms() -> i64 {
    configured_seconds(PLAN_USAGE_TTL_ENV, DEFAULT_PLAN_USAGE_TTL_SECS)
        .saturating_mul(MS_PER_SECOND)
}

/// How long a last good reading is served after it stopped being current.
pub fn plan_usage_retention_ms() -> i64 {
    configured_seconds(PLAN_USAGE_RETENTION_ENV, DEFAULT_PLAN_USAGE_RETENTION_SECS)
        .saturating_mul(MS_PER_SECOND)
}

/// This subscription's own refresh window: the nominal one, spread by up to a
/// quarter either way and stable for a given id.
pub fn jittered_plan_usage_ttl_ms(subscription_id: &str) -> i64 {
    // FNV-1a over the id. A named hash rather than the standard hasher because
    // this number has to mean the same thing in every process that reads the
    // same ledger, and a hasher whose keys may be randomized would not.
    const FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
    const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut hash = FNV_OFFSET_BASIS;
    for byte in subscription_id.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    let spread = 2 * PLAN_USAGE_TTL_JITTER_PERCENT + 1;
    let offset = i64::try_from(hash % u64::try_from(spread).unwrap_or(1)).unwrap_or_default();
    let factor = PERCENT - PLAN_USAGE_TTL_JITTER_PERCENT + offset;
    plan_usage_ttl_ms().saturating_mul(factor) / PERCENT
}

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

/// Where one plan reading came from.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum UsageSource {
    /// The provider's own usage report, read without spending any quota. The
    /// strongest of the three: it is the vendor's current statement about the
    /// account, asked for on purpose.
    Provider,
    /// The headers of a request a caller actually made. Authoritative when it
    /// arrives and silent otherwise, which is why it cannot be the only source.
    Traffic,
    /// An operator's on-demand probe, which spends one minimal completion.
    Probe,
}

impl UsageSource {
    /// The stored name, which is also the name every reader sees.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Provider => "provider",
            Self::Traffic => "traffic",
            Self::Probe => "probe",
        }
    }
}

/// Which kind of check produced a verdict.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CheckSource {
    /// The provider's own usage report. Costs no quota, so this is what runs on
    /// a timer.
    UsageReport,
    /// One minimal completion, which costs quota and therefore only ever runs
    /// when an operator asks for it.
    Completion,
}

/// The outcome of the newest proactive check of one subscription.
///
/// `ok` is about the provider call, not about the plan: a provider that answered
/// and publishes no window at all is a successful check with no readings, and
/// that is precisely the state no reader could name before. Both kinds of check
/// write here, because what a reader needs is the newest verdict about the
/// account rather than the newest verdict of one particular mechanism; `source`
/// says which one it was.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Probe {
    pub attempted_at_ms: i64,
    pub ok: bool,
    /// The provider's own sentence when it refused, trimmed like every other
    /// stored reason here. On success it is set only when the success itself
    /// needs explaining -- a provider that publishes no usage report at all --
    /// and absent otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// Which check this verdict came from. Absent in ledgers written before the
    /// free usage report existed, where every verdict was a completion.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<CheckSource>,
}

/// Where one subscription's credential stands with its provider.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CredentialState {
    /// Nothing has refused this grant, as far as anything has observed.
    #[default]
    Active,
    /// A refresh was refused definitively. No retry repairs this one; only a
    /// sign-in that replaces the stored grant does.
    NeedsReauthorization,
    /// An operator or a lifecycle retired this subscription.
    Disabled,
}

impl CredentialState {
    /// The stored name, which is also the name every reader sees.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::NeedsReauthorization => "needs_reauthorization",
            Self::Disabled => "disabled",
        }
    }

    /// Whether a credential in this state is worth presenting to a provider.
    pub fn usable(self) -> bool {
        matches!(self, Self::Active)
    }
}

/// What is known about one subscription's grant.
///
/// Every field is defaulted on read, because a ledger written before this
/// record existed is the normal case on the first start after an upgrade and
/// must keep loading.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Credential {
    #[serde(default)]
    pub state: CredentialState,
    /// The provider's own sentence for a refusal, trimmed like every other
    /// stored reason here. Absent while the credential is accepted, because a
    /// stale refusal beside a working grant is what sends an operator looking
    /// for a sign-in that is not needed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cause: Option<String>,
    /// When this state was established.
    #[serde(default)]
    pub recorded_at_ms: i64,
    /// When the provider says the access token stops working, when the
    /// credential states it at all. An API key states nothing and stays absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at_ms: Option<i64>,
    /// When a refresh last replaced this grant. Reading a still-valid token
    /// does not move it: the question it answers is when the last rotation
    /// happened, not when anything last looked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refreshed_at_ms: Option<i64>,
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
    record_call_from(subscription_id, provider, response, UsageSource::Traffic);
}

/// The same, saying which kind of call this was.
///
/// A caller's request and an operator's probe both measure real spend and both
/// carry the provider's plan headers, so they share every line of this. They are
/// not the same statement about the plan, though: one is a window the account
/// happened to reveal while working, the other a window somebody asked for, and
/// a row that cannot say which cannot explain why a reading exists.
pub fn record_call_from(
    subscription_id: &str,
    provider: &str,
    response: &ModelResponse,
    source: UsageSource,
) {
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
        // An answer that carried no window says nothing about where the newest
        // one came from, so the recorded source stays as it was.
        if !response.limits.is_empty() {
            entry.usage_source = Some(source);
        }
        // A window that is no longer exhausted clears the block without waiting
        // for its deadline: the provider just answered, which is better evidence
        // than the estimate that produced the block.
        if response.success {
            entry.block = None;
            // An answer is also proof that the grant behind it is accepted, so a
            // recorded refusal is over. This is what lets a credential repaired
            // outside Brama -- the renewal loop writes the vault item directly --
            // come back on its own instead of reading as needing a sign-in
            // forever while it works. A retirement is not cleared here: one
            // stray success must not un-retire a subscription somebody retired.
            if let Some(credential) = entry.credential.as_mut() {
                if credential.state == CredentialState::NeedsReauthorization {
                    credential.state = CredentialState::Active;
                    credential.cause = None;
                    credential.recorded_at_ms = now;
                }
            }
        }
    });
}

/// Record what the provider's own usage report said about one subscription.
///
/// This is the ordinary path now: it costs no quota, so it runs on a timer, and
/// the readings land in exactly the map real traffic writes to, keyed by the same
/// limit ids. A report that carried no window is still a successful check --
/// which is what tells a reader that the blank row is the provider's answer and
/// not a broken credential.
pub fn record_plan_usage(subscription_id: &str, provider: &str, readings: &[LimitReading]) {
    let now = now_ms();
    with_ledger(|ledger| {
        let entry = ledger
            .subscriptions
            .entry(subscription_id.to_string())
            .or_default();
        entry.provider = provider.to_string();
        entry.updated_at_ms = Some(now);
        entry.plan_usage_checked_at_ms = Some(now);
        for reading in readings {
            entry
                .limits
                .insert(reading.limit_id.clone(), reading.clone());
        }
        if !readings.is_empty() {
            entry.usage_source = Some(UsageSource::Provider);
        }
        entry.probe = Some(Probe {
            attempted_at_ms: now,
            ok: true,
            detail: None,
            source: Some(CheckSource::UsageReport),
        });
    });
}

/// Record that this provider publishes no usage report at all.
///
/// A fact about the provider, not a failure of the reader, and one worth storing:
/// without it an operator looking at a blank plan cannot tell a vendor that
/// states nothing from a gateway that never asked.
pub fn record_plan_usage_unpublished(subscription_id: &str, provider: &str, detail: &str) {
    let now = now_ms();
    let detail = bounded_reason(Some(detail));
    with_ledger(|ledger| {
        let entry = ledger
            .subscriptions
            .entry(subscription_id.to_string())
            .or_default();
        entry.provider = provider.to_string();
        entry.updated_at_ms = Some(now);
        entry.plan_usage_checked_at_ms = Some(now);
        entry.probe = Some(Probe {
            attempted_at_ms: now,
            ok: true,
            detail,
            source: Some(CheckSource::UsageReport),
        });
    });
}

/// Record that the provider refused to state this subscription's usage.
///
/// The readings already stored are left exactly where they are. A refusal is a
/// reason the row is not current, not evidence that the last good reading was
/// wrong, and replacing it with nothing would blank a screen over one bad
/// minute upstream.
pub fn record_plan_usage_refused(subscription_id: &str, provider: &str, detail: &str) {
    let now = now_ms();
    let detail = bounded_reason(Some(detail));
    with_ledger(|ledger| {
        let entry = ledger
            .subscriptions
            .entry(subscription_id.to_string())
            .or_default();
        entry.provider = provider.to_string();
        entry.updated_at_ms = Some(now);
        entry.plan_usage_checked_at_ms = Some(now);
        entry.probe = Some(Probe {
            attempted_at_ms: now,
            ok: false,
            detail,
            source: Some(CheckSource::UsageReport),
        });
    });
}

/// Trim one stored sentence to the bound every reason in this file shares.
fn bounded_reason(detail: Option<&str>) -> Option<String> {
    detail
        .map(str::trim)
        .filter(|detail| !detail.is_empty())
        .map(|detail| detail.chars().take(REASON_LIMIT).collect::<String>())
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

/// When a refusal's verdict was ESTABLISHED, given what the ledger already
/// held and the refusal being recorded now.
///
/// `recorded_at_ms` is not "when this was last restated", and the difference
/// is load-bearing: the renewal sweep compares it against the browser sign-in
/// already spent on the credential, so restamping an identical refusal reads
/// as new information and buys one more real sign-in. On 2026-09-02 a single
/// operator-forced model call -- which re-records the same refusal on its way
/// to failing -- was enough to reopen the loop the sweep gate had just closed.
///
/// A refusal whose state and provider sentence are unchanged keeps the instant
/// it was first established. Anything else is a new verdict and gets now: a
/// different sentence from the provider is a different statement about the
/// account, and a credential that had gone back to `active` in between has a
/// genuinely new refusal even if the sentence repeats.
fn refusal_recorded_at_ms(previous: Option<&Credential>, cause: &str, now: i64) -> i64 {
    match previous {
        Some(credential)
            if credential.state == CredentialState::NeedsReauthorization
                && credential.cause.as_deref() == Some(cause) =>
        {
            credential.recorded_at_ms
        }
        _ => now,
    }
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
    let cause: String = reason.chars().take(REASON_LIMIT).collect();
    with_ledger(|ledger| {
        let entry = ledger
            .subscriptions
            .entry(subscription_id.to_string())
            .or_default();
        entry.provider = provider.to_string();
        entry.updated_at_ms = Some(now);
        // The block stops the credential being spent for the next half hour;
        // the state is what says a sign-in, not a wait, is the repair. Both are
        // written because they answer different questions and one of them was
        // missing for as long as it took to lose five days.
        let expires_at_ms = entry
            .credential
            .as_ref()
            .and_then(|credential| credential.expires_at_ms);
        let refreshed_at_ms = entry
            .credential
            .as_ref()
            .and_then(|credential| credential.refreshed_at_ms);
        let previous = entry.credential.take();
        entry.credential = Some(Credential {
            state: CredentialState::NeedsReauthorization,
            cause: Some(cause.clone()),
            recorded_at_ms: refusal_recorded_at_ms(previous.as_ref(), &cause, now),
            expires_at_ms,
            refreshed_at_ms,
        });
        entry.block = Some(Block {
            blocked_until_ms: now.saturating_add(REAUTHORIZATION_BLOCK_MS),
            reason: cause,
            recorded_at_ms: now,
            envelope: Some(recorded.to_json()),
        });
    });
}

/// Record that this subscription's grant is in working order, and until when.
///
/// `refreshed` separates a grant this call replaced from one it only read.
/// Both prove the provider still accepts the credential, but only the first is
/// a rotation, and an operator asking why a token died early needs to know
/// which of the two the instant describes.
///
/// A call that changes nothing leaves the record untouched, including the
/// record's own `updated_at_ms`. A sweep that confirms what the ledger already
/// says has observed nothing new, and stamping it every minute would make every
/// row look freshly seen and leave no reading ever stale.
pub fn record_credential_active(
    subscription_id: &str,
    provider: &str,
    expires_at_ms: Option<i64>,
    refreshed: bool,
) {
    let now = now_ms();
    with_ledger(|ledger| {
        let entry = ledger
            .subscriptions
            .entry(subscription_id.to_string())
            .or_default();
        let previous = entry.credential.take();
        let unchanged = !refreshed
            && previous.as_ref().is_some_and(|credential| {
                credential.state == CredentialState::Active
                    && credential.cause.is_none()
                    && credential.expires_at_ms == expires_at_ms
            });
        if unchanged {
            entry.credential = previous;
            return;
        }
        // A grant that just refreshed is proof the provider accepts it, so the
        // half-hour block a refusal left behind has been outlived by evidence.
        // Only that block is cleared: a rate limit is the provider's own
        // schedule and is none of this record's business.
        if previous
            .as_ref()
            .is_some_and(|credential| credential.state == CredentialState::NeedsReauthorization)
        {
            entry.block = None;
        }
        entry.provider = provider.to_string();
        entry.updated_at_ms = Some(now);
        entry.credential = Some(Credential {
            state: CredentialState::Active,
            cause: None,
            recorded_at_ms: now,
            expires_at_ms,
            refreshed_at_ms: if refreshed {
                Some(now)
            } else {
                previous.and_then(|credential| credential.refreshed_at_ms)
            },
        });
    });
}

/// Record that a sign-in stored a new credential for this subscription.
///
/// This is the only thing that repairs a `needs_reauthorization`, so it clears
/// the cause and the block the refusal left. The previous grant's expiry and
/// rotation instants are dropped rather than kept: they describe a credential
/// that is no longer the one in the vault.
pub fn record_credential_signed_in(subscription_id: &str, provider: &str) {
    let now = now_ms();
    with_ledger(|ledger| {
        let entry = ledger
            .subscriptions
            .entry(subscription_id.to_string())
            .or_default();
        if entry
            .credential
            .as_ref()
            .is_some_and(|credential| credential.state == CredentialState::NeedsReauthorization)
        {
            entry.block = None;
        }
        entry.provider = provider.to_string();
        entry.updated_at_ms = Some(now);
        entry.credential = Some(Credential {
            state: CredentialState::Active,
            cause: None,
            recorded_at_ms: now,
            expires_at_ms: None,
            refreshed_at_ms: None,
        });
    });
}

/// Record that this subscription was retired, with the reason it was.
pub fn record_credential_disabled(subscription_id: &str, provider: &str, cause: &str) {
    let now = now_ms();
    with_ledger(|ledger| {
        let entry = ledger
            .subscriptions
            .entry(subscription_id.to_string())
            .or_default();
        let expires_at_ms = entry
            .credential
            .as_ref()
            .and_then(|credential| credential.expires_at_ms);
        let refreshed_at_ms = entry
            .credential
            .as_ref()
            .and_then(|credential| credential.refreshed_at_ms);
        entry.provider = provider.to_string();
        entry.updated_at_ms = Some(now);
        entry.credential = Some(Credential {
            state: CredentialState::Disabled,
            cause: Some(cause.chars().take(REASON_LIMIT).collect()),
            recorded_at_ms: now,
            expires_at_ms,
            refreshed_at_ms,
        });
    });
}

/// What the ledger alone says about one subscription's grant, before anything is
/// read from the vault.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RefreshHint {
    /// The provider disowned this grant, or it was retired. Leave it alone: no
    /// refresh can repair it and only a sign-in replaces it.
    AwaitingSignIn,
    /// The recorded expiry is further away than the window asked about, so
    /// nothing is due.
    NotDue,
    /// Nothing recorded rules a refresh out. Read the credential and decide from
    /// what it says.
    Read,
}

/// What the ledger already knows about whether this grant needs refreshing
/// within `within`.
///
/// One ledger read answers both questions a sweep asks, because each of them
/// otherwise costs a load of its own. And answering them from here at all is
/// what keeps a sweep cheap: reading the credential means shelling out to the
/// entitlements router, and doing that once a minute per subscription to
/// re-read an expiry this file already holds is the sort of cost that gets a
/// background task switched off.
///
/// Silence is never evidence. A subscription with no recorded credential
/// answers [`RefreshHint::Read`], so a host whose ledger file was just created
/// refreshes normally instead of skipping every account on it.
pub fn credential_refresh_hint(subscription_id: &str, within: Duration) -> RefreshHint {
    let horizon = now_ms().saturating_add(i64::try_from(within.as_millis()).unwrap_or(i64::MAX));
    with_ledger(|ledger| {
        let Some(credential) = ledger
            .subscriptions
            .get(subscription_id)
            .and_then(|entry| entry.credential.as_ref())
        else {
            return RefreshHint::Read;
        };
        if !credential.state.usable() {
            return RefreshHint::AwaitingSignIn;
        }
        match credential.expires_at_ms {
            // A recorded expiry beyond the horizon is this credential's own
            // statement about itself, written the last time it was read or
            // refreshed. A grant replaced outside Brama can make it wrong, and
            // the request path's own refresh is what covers that case.
            Some(expires_at_ms) if expires_at_ms > horizon => RefreshHint::NotDue,
            _ => RefreshHint::Read,
        }
    })
}

/// When the ledger's verdict about one subscription's grant was recorded, or
/// `None` when nothing has ever been recorded about it.
///
/// This is the instant a repair loop compares itself against. A browser
/// sign-in can only change the answer if the stored credential changed, and the
/// ledger writes a new instant every time it does -- a refusal, a rotation, a
/// sign-in that stored something. A verdict no newer than the last sign-in
/// therefore proves that sign-in has already been tried against exactly this
/// state and produced this.
pub fn credential_recorded_at_ms(subscription_id: &str) -> Option<i64> {
    with_ledger(|ledger| {
        ledger
            .subscriptions
            .get(subscription_id)
            .and_then(|entry| entry.credential.as_ref())
            .map(|credential| credential.recorded_at_ms)
    })
}

/// Record what an operator's on-demand completion probe learned about one
/// subscription.
///
/// The probe answers the one question a free usage report cannot: whether the
/// provider will actually serve this credential. It costs a completion, so it
/// runs only when somebody asks, and the verdict it returns is handed straight
/// back to whoever asked. Any readings the probe's answer carried are recorded by
/// [`record_call_from`] on the same path real traffic takes; this only adds the
/// verdict.
pub fn record_probe(
    subscription_id: &str,
    provider: &str,
    ok: bool,
    detail: Option<&str>,
) -> Probe {
    let now = now_ms();
    let probe = Probe {
        attempted_at_ms: now,
        ok,
        detail: bounded_reason(detail),
        source: Some(CheckSource::Completion),
    };
    let recorded = probe.clone();
    with_ledger(|ledger| {
        let entry = ledger
            .subscriptions
            .entry(subscription_id.to_string())
            .or_default();
        entry.provider = provider.to_string();
        entry.updated_at_ms = Some(now);
        entry.probe = Some(probe);
    });
    recorded
}

/// Whether this subscription's usage report is due to be read again.
///
/// Two windows have to have passed, and they answer different questions. The
/// report was checked longer ago than this subscription's own spread cache
/// window -- that is the cache -- and no reading of any kind is younger than it,
/// so a row that traffic just refreshed does not spend a request on a provider
/// that rate-limits usage reads per address to learn what it already knows.
pub fn plan_usage_due(subscription_id: &str) -> bool {
    let now = now_ms();
    let window = jittered_plan_usage_ttl_ms(subscription_id);
    with_ledger(|ledger| {
        let Some(entry) = ledger.subscriptions.get(subscription_id) else {
            return true;
        };
        let checked_recently = entry
            .plan_usage_checked_at_ms
            .is_some_and(|checked| now.saturating_sub(checked) < window);
        let read_recently =
            newest_reading_ms(entry).is_some_and(|recorded| now.saturating_sub(recorded) < window);
        !checked_recently && !read_recently
    })
}

/// The instant of the newest plan reading this subscription holds.
fn newest_reading_ms(entry: &SubscriptionUsage) -> Option<i64> {
    entry
        .limits
        .values()
        .map(|reading| reading.recorded_at_ms)
        .filter(|recorded| *recorded > 0)
        .max()
}

/// The plan windows a reader should be shown, where they came from, and whether
/// the newest of them has aged past the freshness window.
pub struct PlanWindows {
    pub limits: Vec<LimitReading>,
    pub source: Option<UsageSource>,
    pub stale: bool,
}

/// Project one subscription's readings for a reader.
///
/// A stale reading is served, with `stale` set, because a reading that states
/// when it was taken is information and an empty plan is not -- an upstream that
/// fails for ten minutes must not blank a row that was right ten minutes ago.
/// Past the retention window it stops being served: a fraction of a five-hour
/// window that has since reset four times describes nothing, and a reader has no
/// way to know that from the number alone.
pub fn plan_windows(usage: Option<&SubscriptionUsage>) -> PlanWindows {
    let Some(usage) = usage else {
        return PlanWindows {
            limits: Vec::new(),
            source: None,
            stale: false,
        };
    };
    let now = now_ms();
    let retention = plan_usage_retention_ms();
    let limits = usage
        .limits
        .values()
        .filter(|reading| {
            // A reading with no instant of its own cannot be aged, and the load
            // path has already given every one of those the tightest upper bound
            // available, so anything still at zero is served rather than judged.
            reading.recorded_at_ms == 0 || now.saturating_sub(reading.recorded_at_ms) <= retention
        })
        .cloned()
        .collect::<Vec<_>>();
    if limits.is_empty() {
        return PlanWindows {
            limits,
            source: None,
            stale: false,
        };
    }
    let newest = limits
        .iter()
        .map(|reading| reading.recorded_at_ms)
        .max()
        .unwrap_or_default();
    PlanWindows {
        stale: newest > 0 && now.saturating_sub(newest) > plan_usage_ttl_ms(),
        limits,
        source: usage.usage_source,
    }
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

/// Whether this subscription's recorded block is an authorization block.
///
/// [`record_reauthorization_needed`] writes two things: the state that says a
/// sign-in is the repair, and a half-hour block that stops the credential being
/// spent meanwhile. The router skips a blocked credential without asking the
/// provider, so for that half hour the only record of *why* the pool is empty
/// is this state - and a caller told the pool was merely bounded is told to
/// wait for something no wait reaches. This is how the request path reads the
/// difference.
pub fn needs_reauthorization(subscription_id: &str) -> bool {
    with_ledger(|ledger| {
        ledger
            .subscriptions
            .get(subscription_id)
            .and_then(|entry| entry.credential.as_ref())
            .is_some_and(|credential| credential.state == CredentialState::NeedsReauthorization)
    })
}

/// The earliest future reset instant across this subscription's served windows.
///
/// A pin on this credential should die with its tightest window, and this is
/// that window's end. `None` means no served reading names a future reset, so
/// the caller falls back to its own default rather than treating the pin as
/// immortal.
pub fn next_reset_ms(subscription_id: &str) -> Option<i64> {
    let entry = usage_for(subscription_id)?;
    let windows = plan_windows(Some(&entry));
    let now = now_ms();
    windows
        .limits
        .iter()
        .filter_map(|reading| reading.resets_at_ms)
        .filter(|resets_at_ms| *resets_at_ms > now)
        .min()
}

/// How much of this subscription's tightest current plan window is spent.
///
/// This is the routing view of the ledger: the maximum used fraction across
/// the windows [`plan_windows`] still serves, with one adjustment -- a window
/// whose own reset instant has passed counts as empty, because the provider's
/// clock says it rolled and charging its last reading against the account
/// would freeze a credential that is free again. Readings past the retention
/// window are already absent from the projection, so they cannot route
/// anything either.
///
/// `None` means nothing usable is recorded, which is not the same statement as
/// a known-empty plan: it covers a subscription no traffic ever reached and a
/// provider that publishes no windows at all. Callers placing candidates treat
/// it as fully available, because the first real call writes the reading that
/// corrects the placement.
pub fn used_fraction(subscription_id: &str) -> Option<f64> {
    let entry = usage_for(subscription_id)?;
    let windows = plan_windows(Some(&entry));
    if windows.limits.is_empty() {
        return None;
    }
    let now = now_ms();
    Some(
        windows
            .limits
            .iter()
            .map(|reading| match reading.resets_at_ms {
                Some(resets_at_ms) if resets_at_ms <= now => 0.0,
                _ => reading.used_fraction,
            })
            .fold(0.0_f64, f64::max),
    )
}

/// The recorded state of one subscription, if anything was ever recorded.
pub fn usage_for(subscription_id: &str) -> Option<SubscriptionUsage> {
    with_ledger(|ledger| ledger.subscriptions.get(subscription_id).cloned())
}

/// Every subscription this ledger file describes, read without writing it back.
///
/// Deliberately not routed through [`with_ledger`], which persists after every
/// call including a read. An operator listing the pool while the gateway is
/// serving would otherwise rewrite the file from a snapshot taken before the
/// gateway's own next write, so looking at the ledger could lose a record --
/// and looking is the whole purpose of the command that calls this.
///
/// The ledger is asked to enumerate rather than to answer about one id because
/// an operator diagnosing an empty pool does not know which subscriptions
/// exist; that is part of what they are asking.
pub fn recorded_subscriptions() -> BTreeMap<String, SubscriptionUsage> {
    load().subscriptions
}

#[cfg(test)]
mod tests {
    use super::{refusal_recorded_at_ms, Credential, CredentialState};

    /// 2026-08-27T00:24:00Z, when codex's grant was first refused.
    const ESTABLISHED_MS: i64 = 1_787_790_240_000;
    const SENTENCE: &str = "OAuth refresh rejected with HTTP 401: Your session has ended. \
                            Please log in again.";

    fn refused(cause: &str, recorded_at_ms: i64) -> Credential {
        Credential {
            state: CredentialState::NeedsReauthorization,
            cause: Some(cause.to_string()),
            recorded_at_ms,
            expires_at_ms: None,
            refreshed_at_ms: None,
        }
    }

    /// The defect: re-recording the same refusal restamped the verdict, which
    /// the sweep reads as new information and answers with a browser sign-in.
    #[test]
    fn restating_an_identical_refusal_keeps_the_original_instant() {
        let previous = refused(SENTENCE, ESTABLISHED_MS);
        let much_later = ESTABLISHED_MS + 6 * 24 * 60 * 60 * 1000;
        assert_eq!(
            refusal_recorded_at_ms(Some(&previous), SENTENCE, much_later),
            ESTABLISHED_MS,
            "an identical refusal restated later is not a new verdict"
        );
    }

    /// A different sentence from the provider IS a new statement about the
    /// account, and one automatic sign-in against it is the design.
    #[test]
    fn a_different_provider_sentence_is_a_new_verdict() {
        let previous = refused(SENTENCE, ESTABLISHED_MS);
        let now = ESTABLISHED_MS + 1000;
        assert_eq!(
            refusal_recorded_at_ms(Some(&previous), "invalid_grant", now),
            now
        );
    }

    /// A credential that had gone back to active has a genuinely new refusal
    /// even when the sentence repeats.
    #[test]
    fn a_refusal_after_a_working_credential_is_a_new_verdict() {
        let previous = Credential {
            state: CredentialState::Active,
            cause: None,
            recorded_at_ms: ESTABLISHED_MS,
            expires_at_ms: None,
            refreshed_at_ms: None,
        };
        let now = ESTABLISHED_MS + 1000;
        assert_eq!(refusal_recorded_at_ms(Some(&previous), SENTENCE, now), now);
    }

    /// The first refusal ever recorded establishes the verdict.
    #[test]
    fn a_first_refusal_establishes_the_verdict() {
        assert_eq!(
            refusal_recorded_at_ms(None, SENTENCE, ESTABLISHED_MS),
            ESTABLISHED_MS
        );
    }
}
