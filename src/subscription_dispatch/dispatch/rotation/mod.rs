//! One route, walked across the bounded credential pool an agent holds for its
//! provider.
//!
//! `roster` decides which accounts this call may be billed to and in what
//! order; `buffered` and `streaming` each spend that roster on one provider,
//! rotating only while the caller has seen nothing; `verdict` writes the single
//! refusal an emptied pool is reported with, so the two paths cannot answer the
//! same broken credential differently -- a quota reset still takes precedence
//! over an account needing authorization, on both.

use crate::types::ModelResponse;

pub(super) mod buffered;
pub(super) mod roster;
pub(super) mod streaming;
pub(super) mod verdict;

/// One provider's answer to one candidate route, and whether the refusal
/// emptied that provider's whole pool for this agent rather than failing this
/// one route.
///
/// A ranked walk needs the distinction and cannot recover it from the sentence:
/// an emptied pool refuses every remaining route of the provider identically,
/// while a route the provider rejected on its own merits says nothing about the
/// provider's other models.
pub(in crate::subscription_dispatch::dispatch) struct RouteAttempt<T> {
    pub(in crate::subscription_dispatch::dispatch) opened: Result<T, ModelResponse>,
    pub(in crate::subscription_dispatch::dispatch) pool_emptied: bool,
}

impl<T> RouteAttempt<T> {
    fn served(opened: T) -> Self {
        Self {
            opened: Ok(opened),
            pool_emptied: false,
        }
    }

    /// A refusal that belongs to this route alone.
    fn refused(failure: ModelResponse) -> Self {
        Self {
            opened: Err(failure),
            pool_emptied: false,
        }
    }

    /// A refusal that is this provider's pool having nothing left for the agent.
    fn pool_empty(failure: ModelResponse) -> Self {
        Self {
            opened: Err(failure),
            pool_emptied: true,
        }
    }
}
