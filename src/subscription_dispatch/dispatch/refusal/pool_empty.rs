//! Why an emptied credential pool emptied, and the one sentence each cause is
//! reported with.

/// Which kind an emptied credential pool is, so the log envelope and the HTTP
/// answer say the same thing.
///
/// A provider that refused every credential and a vault that produced none are
/// both authorization failures no wait repairs; only a genuinely exhausted pool
/// is capacity.
///
/// A credential inside an authorization block counts with the first group. The
/// router skips a blocked credential without calling the provider, so a
/// credential the provider had already refused looked, for the half hour its
/// block lasted, exactly like one that was merely out of quota - and the caller
/// was told to retry. That is the same defect as reporting a refused redemption
/// as capacity, arriving one layer further in.
pub(in crate::subscription_dispatch::dispatch) fn rotation_failure_kind(
    cause: PoolEmptyCause,
) -> &'static str {
    if cause.needs_authorization() {
        "credential_unauthorized"
    } else {
        "subscription_unavailable"
    }
}

/// Why an emptied credential pool emptied, as one value.
///
/// The buffered and streaming paths reach this decision independently and used
/// to spell it twice. They now share it, because the two spellings drifting is
/// how the same broken credential comes to answer `503` to one caller and `429`
/// to another.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PoolEmptyCause {
    /// A provider refused a credential outright during this request.
    pub auth_rejection: bool,
    /// A credential was skipped because its recorded block is an authorization
    /// block: the same refusal, still being served from the ledger.
    pub reauthorization_block: bool,
    /// The vault produced no credential at all -- no capability, no grant.
    pub unredeemable_credential: bool,
}

impl PoolEmptyCause {
    /// Whether repairing this needs an authorization, not a wait.
    pub fn needs_authorization(self) -> bool {
        self.auth_rejection || self.reauthorization_block || self.unredeemable_credential
    }
}

/// Whether an emptied pool is capacity, given everything one walk saw.
///
/// `rate_limit_block` is set per credential, and only for a credential whose
/// recorded block is a rate limit and not an authorization block — the walk
/// reads the ledger for each one. So its presence means one member of this
/// pool is otherwise usable and merely out of quota, and a caller who waits
/// is served by that member whatever the other members need.
///
/// The order used to be the reverse, and both directions have cost a
/// diagnosis. Production answered `429 all bounded 'codex' credentials
/// unavailable for agent`, retryable, while its own ledger recorded that every
/// one of those credentials needed a sign-in: that case sets no
/// `rate_limit_block` at all, so it is authorization here and stays so. On
/// 2026-09-21 the opposite happened — the pool held one live credential at
/// 100% of its seven-day quota, resetting in fourteen hours, beside members
/// burnt by the borrowing this product removed — and the answer was
/// `503 subscription_reauthorization_required`, which reads as a task for a
/// person while the repair was a wait.
pub fn pool_is_capacity(rate_limit_block: bool) -> bool {
    rate_limit_block
}

/// Whether a capacity refusal also has members that need a sign-in, so the
/// sentence can say both: the wait serves this request, and the pool is one
/// account wide until somebody signs the others in.
pub fn capacity_is_mixed(cause: PoolEmptyCause) -> bool {
    cause.needs_authorization()
}

/// The sentence one emptied pool is reported with.
///
/// Four causes, and the caller acts on each differently: a provider that
/// refused needs a sign-in, a credential inside an authorization block is that
/// same refusal still recorded, a vault that produced nothing needs a
/// capability or grant repaired, and everything else is quota worth waiting
/// out. Only the last is retryable.
///
/// The block case is why this is a named function with a test beside it.
/// `codex` answered `401 Your session has ended. Please log in again`, which
/// recorded `needs_reauthorization` and a half-hour block; every request inside
/// that window skipped the credential without asking anyone, emptied the pool
/// with nothing observed, and was reported as capacity anyway. The ledger
/// had recorded the authorization failure the whole time, and the caller was
/// told to retry -- which is the defect ARCHITECTURE.md records as fixed,
/// reappearing one layer further in.
pub fn pool_empty_summary(provider: &str, cause: PoolEmptyCause) -> String {
    if cause.auth_rejection || cause.reauthorization_block {
        auth_rejected_summary(provider)
    } else if cause.unredeemable_credential {
        unredeemable_credential_summary(provider)
    } else {
        bounded_unavailable_summary(provider)
    }
}

/// The request path's own sentence for a pool whose credentials the vault
/// would not produce.
///
/// These three sentences are what an operator reads when a call is refused, and
/// a readiness check that invented its own wording for the same fault would
/// make one broken chain look like two. They are written once here and used by
/// the buffered path, the streaming path, and
/// [`probe_subscription_redemption`](crate::subscription_dispatch::dispatch::probe_subscription_redemption).
fn unredeemable_credential_summary(provider: &str) -> String {
    format!(
        "no '{provider}' credential could be redeemed for agent; a capability, \
         read grant, or this installation's trust material is missing"
    )
}

/// The request path's own sentence for a pool every one of whose credentials is
/// inside a recorded rate-limit block.
pub(in crate::subscription_dispatch::dispatch) fn bounded_unavailable_summary(
    provider: &str,
) -> String {
    format!("all bounded '{provider}' credentials unavailable for agent")
}

/// The capacity sentence for a pool whose usable member is out of quota while
/// other members need a sign-in. Both facts travel, because the wait serves
/// this request and the pool is still one account wide.
pub(in crate::subscription_dispatch::dispatch) fn mixed_unavailable_summary(
    provider: &str,
) -> String {
    format!(
        "all bounded '{provider}' credentials unavailable for agent: the usable one is inside a \
         quota block, and the rest need a sign-in (`brama subscriptions` says which)"
    )
}

/// The whole capacity sentence: which members are unavailable, whether the
/// rest need a sign-in, and the hour the wait ends when the ledger knows it.
///
/// It used to end at "lifts on its own", pointing the reader at `brama
/// subscriptions`, which prints a block's reason and not its end. On
/// 2026-09-21 a judge's route was refused all day and no read anywhere named
/// an hour, while `blocked_until_ms` had been in the ledger the whole time.
pub fn capacity_summary(provider: &str, mixed: bool, block_lifts_at_ms: Option<i64>) -> String {
    let summary = if mixed {
        mixed_unavailable_summary(provider)
    } else {
        bounded_unavailable_summary(provider)
    };
    match block_lifts_at_ms {
        Some(until) => format!("{summary}; the block lifts at {}", block_instant(until)),
        None => summary,
    }
}

/// One ledger instant as a reader can act on it: UTC, to the second.
fn block_instant(milliseconds: i64) -> String {
    chrono::DateTime::from_timestamp_millis(milliseconds)
        .map(|at| at.format("%Y-%m-%dT%H:%M:%SZ").to_string())
        .unwrap_or_else(|| format!("{milliseconds}ms"))
}

/// The request path's own sentence for a pool every one of whose credentials
/// the provider itself refused.
///
/// It used to end in "re-authorization required", which reads as a task for a
/// person while re-authorization is the refresh sweep's own job. The sentence
/// now carries what that sweep did for each of the provider's subscriptions,
/// so the reader learns whether it never ran, failed and on what, or succeeded
/// and the provider refused the credential anyway.
pub(in crate::subscription_dispatch::dispatch) fn auth_rejected_summary(provider: &str) -> String {
    format!(
        "all bounded '{provider}' credentials were rejected by the provider; {}",
        crate::subscription_dispatch::sign_in::automatic_sign_in_sentence(provider)
    )
}

/// The request path's own sentence for an agent with no active credential of
/// this provider at all -- the answer a subscription whose vault item lost its
/// `brama:agent:` tag produces, because discovery can no longer see it.
pub(crate) fn no_active_credential_summary(provider: &str) -> String {
    format!("no active '{provider}' credential for agent")
}
