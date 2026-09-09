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

/// The request path's own sentence for a pool every one of whose credentials
/// the provider itself refused.
pub(in crate::subscription_dispatch::dispatch) fn auth_rejected_summary(provider: &str) -> String {
    format!(
        "all bounded '{provider}' credentials were rejected by the provider; \
         re-authorization required"
    )
}

/// The request path's own sentence for an agent with no active credential of
/// this provider at all -- the answer a subscription whose vault item lost its
/// `brama:agent:` tag produces, because discovery can no longer see it.
pub(crate) fn no_active_credential_summary(provider: &str) -> String {
    format!("no active '{provider}' credential for agent")
}
