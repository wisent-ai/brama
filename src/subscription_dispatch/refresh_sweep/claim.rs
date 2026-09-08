//! Which subscriptions have a refresh running right now, so that a slow one is
//! never started twice.
//!
//! A subscription two agents share is one account with one grant, and a second
//! attempt against it rotates a refresh token the first attempt has already
//! replaced. The rotation lock in the broker serialises the writes; this claim
//! is what stops a sweep from queueing behind its own previous attempt, and
//! what stops a browser sign-in from being driven while another one is still
//! open for the same subscription.
//!
//! It is a guard rather than a flag on purpose, and that is why it is a type
//! and not two calls: an attempt that dies on a panic or an early return
//! releases its subscription by unwinding, instead of leaving it claimed for
//! the life of the process. Both the silent refresh and the browser sign-in
//! take their claim from here, which is the only reason this is a module of its
//! own rather than a detail of either one.

use std::collections::HashSet;
use std::sync::{LazyLock, Mutex};

/// The subscriptions a refresh is running for right now.
static IN_FLIGHT: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(|| Mutex::new(HashSet::new()));

/// One subscription claimed for the duration of one refresh.
///
/// Held for the whole attempt and released by dropping, so a refresh that dies
/// on a panic or an early return does not leave its subscription claimed for the
/// life of the process.
pub(super) struct InFlight(String);

impl InFlight {
    /// Claim this subscription, or nothing when a refresh for it is already
    /// running.
    pub(super) fn claim(subscription_id: &str) -> Option<Self> {
        let mut claimed = match IN_FLIGHT.lock() {
            Ok(claimed) => claimed,
            Err(poisoned) => poisoned.into_inner(),
        };
        claimed
            .insert(subscription_id.to_owned())
            .then(|| Self(subscription_id.to_owned()))
    }
}

impl Drop for InFlight {
    fn drop(&mut self) {
        let mut claimed = match IN_FLIGHT.lock() {
            Ok(claimed) => claimed,
            Err(poisoned) => poisoned.into_inner(),
        };
        claimed.remove(&self.0);
    }
}
