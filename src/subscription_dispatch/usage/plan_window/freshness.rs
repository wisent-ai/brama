//! How long a plan reading counts as current, how long a lapsed one is still
//! worth serving, and how far apart two accounts on one host come due.
//!
//! Three durations decide when the ledger stops believing itself, and each is
//! answerable in one sentence about a cost. The first is how often a provider
//! may be asked, which its per-address rate limit has an opinion about. The
//! second is how long a reading that went stale is still better than a blank
//! row. The third is the spread that keeps several accounts sharing one address
//! from coming due in the same second.
//!
//! They are stated here, apart from the readings themselves, because they are
//! configuration an operator may move and every judgement about a reading's
//! currency has to consult them.

/// How long one subscription's plan reading is treated as current.
///
/// Five minutes is short enough that a five-hour window never ages past its own
/// reset unnoticed, and long enough that seven accounts polling their providers
/// cost seven requests per five minutes rather than one per console refresh.
/// Provider usage endpoints may rate-limit per source address.
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
