//! The order a candidate list is walked in: what each route's plan has left,
//! with chance breaking only the ties the ledger says are ties.

use std::io::Read;

use crate::gateway::broker;
use crate::subscription_dispatch::usage;

use super::super::catalogue::route::{provider_for, provider_matches};

fn random_u64() -> Result<u64, String> {
    let mut bytes = u64::default().to_ne_bytes();
    std::fs::File::open("/dev/urandom")
        .and_then(|mut source| source.read_exact(&mut bytes))
        .map_err(|_| "operating system randomness is unavailable".to_string())?;
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
pub(super) fn shuffle_within_equal<T, K: PartialOrd>(
    items: &mut [T],
    key: impl Fn(&T) -> K,
) -> Result<(), String> {
    let mut start = 0;
    while start < items.len() {
        let mut end = start + 1;
        while end < items.len()
            && key(&items[end]).partial_cmp(&key(&items[start])) == Some(std::cmp::Ordering::Equal)
        {
            end += 1;
        }
        let run: &mut [T] = &mut items[start..end];
        if run.len() > 1 {
            for i in (1..run.len()).rev() {
                let j = (random_u64()? as usize) % (i + 1);
                run.swap(i, j);
            }
        }
        start = end;
    }
    Ok(())
}

/// The plan headroom of a route, as the freest usable subscription behind it.
///
/// Routing reads the ledger, never the provider: a number that costs a call to
/// learn cannot be paid for on every selection. A subscription nobody has
/// measured counts as fully available -- its first call writes the reading
/// that corrects that -- and a route with no usable subscription counts as
/// full, so it sorts behind every route that can actually serve.
pub(super) fn route_plan_key(subscriptions: &[broker::SubscriptionEntry], route_id: &str) -> f64 {
    let Some(provider) = provider_for(route_id) else {
        return 1.0;
    };
    let mut best: Option<f64> = None;
    for entry in subscriptions {
        if !provider_matches(&entry.provider, provider)
            || entry.status != "active"
            || crate::journal::is_retired(&entry.id)
            || usage::is_blocked(&entry.id)
        {
            continue;
        }
        let fraction = usage::used_fraction(&entry.id).unwrap_or(0.0);
        best = Some(best.map_or(fraction, |current: f64| current.min(fraction)));
    }
    best.unwrap_or(1.0)
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
) -> Result<(), String> {
    let subscriptions = broker::list_subscriptions(agent_id).await;
    models.sort_by(|left, right| {
        route_plan_key(&subscriptions, left)
            .partial_cmp(&route_plan_key(&subscriptions, right))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    shuffle_within_equal(models, |model| {
        ordered_float_key(route_plan_key(&subscriptions, model))
    })
}

/// An f64 routing key as an orderable integer: sign-flipped bits, so equal
/// fractions compare equal and ties stay shuffles rather than sorts.
pub(super) fn ordered_float_key(value: f64) -> i64 {
    let bits = value.to_bits() as i64;
    if bits < 0 {
        bits ^ i64::MAX
    } else {
        bits
    }
}
