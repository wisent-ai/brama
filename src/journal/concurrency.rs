//! How many calls at once each alias has been seen to carry.
//!
//! A consumer that fans calls out to one alias had to state a worker count
//! nobody could source: too many and the provider refuses for capacity, too
//! few and the work crawls. The gateway sees every call, so it measures the
//! number instead. A call counts as in flight from routing until its provider
//! answers — a buffered answer, or a stream's commit, which is where a
//! provider admits or refuses it. The most calls in flight when one was
//! answered is what the alias has carried; a capacity refusal (the provider's
//! rate limit, or no pool credential free) lowers that to the calls still in
//! flight beside the refused one. `GET /v1/aliases` publishes it per alias as
//! `concurrency`: `carried` (absent while the alias has answered nothing),
//! `refused_beside` (the other calls in flight at the newest capacity
//! refusal, absent when there was none) and `in_flight` now.
//!
//! A call in flight is a live handle the route keeps a weak reference to, so
//! the count is the handles still alive and an abandoned request leaves it by
//! being dropped.

use std::collections::BTreeMap;
use std::sync::{Arc, LazyLock, Mutex, Weak};

use serde::Serialize;

/// One alias's measured concurrency.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Concurrency {
    /// The most calls in flight when one of them was answered, lowered by a
    /// later capacity refusal.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub carried: Option<usize>,
    /// The other calls in flight when the newest capacity refusal came.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refused_beside: Option<usize>,
    /// Calls routed to the alias that its provider has not answered yet.
    pub in_flight: usize,
}

#[derive(Default)]
struct Route {
    calls: Vec<Weak<()>>,
    carried: Option<usize>,
    refused_beside: Option<usize>,
}

impl Route {
    /// The calls still in flight, forgetting the ones that ended.
    fn in_flight(&mut self) -> usize {
        self.calls.retain(|call| call.upgrade().is_some());
        self.calls.len()
    }
}

static ROUTES: LazyLock<Mutex<BTreeMap<String, Route>>> = LazyLock::new(Mutex::default);

/// How a call that was in flight ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Settled {
    /// The provider answered it (a generation, or a stream committed).
    Answered,
    /// The provider or the pool refused it for capacity.
    RefusedForCapacity,
    /// Any other refusal, which says nothing about concurrency.
    Other,
}

/// One call counted in flight for `alias` while it lives.
pub struct Admission {
    alias: String,
    call: Arc<()>,
}

/// Count one call to `alias` in flight.
pub fn admit(alias: &str) -> Admission {
    let call = Arc::new(());
    routes()
        .entry(alias.to_string())
        .or_default()
        .calls
        .push(Arc::downgrade(&call));
    Admission {
        alias: alias.to_string(),
        call,
    }
}

impl Admission {
    /// Take the call out of flight and fold how it ended into the alias's
    /// measurement.
    pub fn settle(self, how: Settled) {
        let Admission { alias, call } = self;
        let mut routes = routes();
        let route = routes.entry(alias).or_default();
        let with_this_one = route.in_flight();
        drop(call);
        let beside = route.in_flight();
        match how {
            Settled::Answered => {
                // The first answer is the first measurement; later ones raise it.
                route.carried = Some(match route.carried {
                    Some(carried) => carried.max(with_this_one),
                    None => with_this_one,
                });
            }
            Settled::RefusedForCapacity => {
                // A refusal before any answer is the first measurement too.
                route.refused_beside = Some(beside);
                route.carried = Some(match route.carried {
                    Some(carried) => carried.min(beside),
                    None => beside,
                });
            }
            Settled::Other => {}
        }
    }
}

/// Every alias's measured concurrency since this gateway started.
pub fn current() -> BTreeMap<String, Concurrency> {
    let mut routes = routes();
    routes
        .iter_mut()
        .map(|(alias, route)| {
            let measured = Concurrency {
                carried: route.carried,
                refused_beside: route.refused_beside,
                in_flight: route.in_flight(),
            };
            (alias.clone(), measured)
        })
        .collect()
}

/// The per-alias measurements. A panic while they were held can leave at most
/// one call's fold unfinished, which the next settlement overwrites, so the
/// lock is recovered: left poisoned, every call would go uncounted and
/// `current` would answer that nothing was ever measured.
fn routes() -> std::sync::MutexGuard<'static, BTreeMap<String, Route>> {
    crate::core::sync::held(&ROUTES, "alias concurrency routes")
}
