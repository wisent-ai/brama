//! The order a candidate list is walked in: what each route's plan has left,
//! with chance breaking only the ties the ledger says are ties.

use std::cmp::Ordering;
use std::io::Read;

use crate::gateway::broker;
use crate::subscription_dispatch::usage;
use crate::types::{GatewayRefusal, Refusal};

use super::super::catalogue::route::{provider_for, provider_matches};

/// Fisher–Yates walks the run from its last index down to the second, and
/// draws each partner from the indices up to and including its own.
const LAST_SWAP: usize = 1; // https://en.wikipedia.org/wiki/Fisher%E2%80%93Yates_shuffle#The_modern_algorithm "for i from n−1 down to 1 do"
const INCLUSIVE: u64 = 1; // https://en.wikipedia.org/wiki/Fisher%E2%80%93Yates_shuffle#The_modern_algorithm "j ← random integer such that 0 ≤ j ≤ i"

/// One draw from the operating system's randomness, or why none was read.
fn random_u64() -> Result<u64, Refusal> {
    let refused = |cause: String| {
        Refusal::gateway(
            GatewayRefusal::DependencyUnavailable,
            format!("reading operating system randomness from /dev/urandom: {cause}"),
        )
    };
    let width = size_of::<u64>();
    let mut bytes = Vec::with_capacity(width);
    std::fs::File::open("/dev/urandom")
        .and_then(|source| source.take(width as u64).read_to_end(&mut bytes))
        .map_err(|error| refused(error.to_string()))?;
    let bytes = bytes
        .as_slice()
        .try_into()
        .map_err(|_| refused(format!("{} bytes read of {width}", bytes.len())))?;
    Ok(u64::from_ne_bytes(bytes))
}

/// Shuffle only inside runs whose key compares equal, preserving the caller's
/// order between runs.
///
/// Random placement exists to decorrelate accounts that would otherwise be
/// hammered in list order, not to overrule a ranking the caller computed. A
/// key derived from plan state says something true about the candidates; an
/// equal key says nothing, and "nothing" is the only thing chance should
/// decide.
pub(in crate::subscription_dispatch) fn shuffle_within_equal<T, K: PartialOrd>(
    items: &mut [T],
    key: impl Fn(&T) -> K,
) -> Result<(), Refusal> {
    for run in items
        .chunk_by_mut(|left, right| key(left).partial_cmp(&key(right)) == Some(Ordering::Equal))
    {
        for i in (LAST_SWAP..run.len()).rev() {
            let bound = i as u64 + INCLUSIVE;
            // Discard the incomplete residue cycle so every index has
            // exactly the same number of accepted random values.
            let threshold = bound.wrapping_neg() % bound;
            let j = loop {
                let value = random_u64()?;
                if value >= threshold {
                    break (value % bound) as usize;
                }
            };
            run.swap(i, j);
        }
    }
    Ok(())
}

/// What a route's or an account's plan has left, as the ledger knows it.
///
/// Routing reads the ledger, never the provider: a number that costs a call
/// to learn cannot be paid for on every selection. A subscription nobody has
/// measured is tried first -- its first call writes the reading that ranks it
/// -- then measured ones from the least used, and a route with no usable
/// subscription last, behind every route that can actually serve.
#[derive(Clone, Copy, Debug)]
pub(in crate::subscription_dispatch) enum Headroom {
    Unmeasured,
    Used(f64),
    Unservable,
}

impl Headroom {
    /// One subscription's headroom from the ledger.
    pub(in crate::subscription_dispatch) fn of(subscription_id: &str) -> Self {
        match usage::used_fraction(subscription_id) {
            Some(fraction) => Self::Used(fraction),
            None => Self::Unmeasured,
        }
    }
}

impl Ord for Headroom {
    fn cmp(&self, other: &Self) -> Ordering {
        match (self, other) {
            (Self::Used(left), Self::Used(right)) => left.total_cmp(right),
            (Self::Unmeasured, Self::Unmeasured) | (Self::Unservable, Self::Unservable) => {
                Ordering::Equal
            }
            (Self::Unmeasured, _) | (_, Self::Unservable) => Ordering::Less,
            (_, Self::Unmeasured) | (Self::Unservable, _) => Ordering::Greater,
        }
    }
}

impl PartialOrd for Headroom {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for Headroom {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for Headroom {}

/// The plan headroom of a route, as the freest usable subscription behind it.
pub(super) fn route_plan_key(
    subscriptions: &[broker::SubscriptionEntry],
    route_id: &str,
) -> Headroom {
    let Some(provider) = provider_for(route_id) else {
        return Headroom::Unservable;
    };
    let freest = subscriptions
        .iter()
        .filter(|entry| {
            provider_matches(&entry.provider, provider)
                && entry.status == "active"
                && !usage::is_blocked(&entry.id)
        })
        .map(|entry| Headroom::of(&entry.id))
        .min();
    match freest {
        Some(headroom) => headroom,
        None => Headroom::Unservable,
    }
}

/// Order candidate models by what their plans have left, fullest window last.
///
/// Selectors used to shuffle the whole list, which spent accounts at random
/// while the ledger already held each one's own statement of how spent it
/// was. The order is now the provider's own numbers; chance only breaks ties,
/// so two accounts at the same utilization still decorrelate and no two
/// accounts at different ones ever trade places.
pub(super) async fn order_models_by_plan(
    agent_id: &str,
    models: &mut [String],
) -> Result<(), Refusal> {
    let subscriptions = unretired(
        broker::routing_subscriptions(agent_id)
            .map_err(|error| Refusal::gateway(GatewayRefusal::DependencyUnavailable, error))?,
    )?;
    models.sort_by_key(|model| route_plan_key(&subscriptions, model));
    shuffle_within_equal(models, |model| route_plan_key(&subscriptions, model))
}

/// The subscriptions whose owner has not retired them, which are the only ones
/// a plan key may count. An unreadable journal is a refusal: read as empty it
/// would rank a route by a member its owner retired.
pub(super) fn unretired(
    subscriptions: Vec<broker::SubscriptionEntry>,
) -> Result<Vec<broker::SubscriptionEntry>, Refusal> {
    let mut kept = Vec::new();
    for entry in subscriptions {
        let retired = crate::journal::is_retired(&entry.id)
            .map_err(|error| Refusal::gateway(GatewayRefusal::DependencyUnavailable, error))?;
        if !retired {
            kept.push(entry);
        }
    }
    Ok(kept)
}
