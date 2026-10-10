//! Which live session runs on which subscription, and how many a
//! subscription carries.
//!
//! Without this, every coding-agent session a machine starts runs on the one
//! account the harness was signed into, and the pool's other members idle
//! while that account's plan is spent by all of them at once; nothing
//! records which session uses which subscription, and a pool whose every
//! member already carries its share of sessions never grows, because the
//! acquisition buys only when every plan is spent.
//!
//! A lease is one session's claim on one subscription: taken before the
//! harness starts, released when the session's terminal ends. The gateway
//! keeps the register, because the pool is the gateway's and every machine's
//! sessions share it. The operator stated how many sessions one subscription
//! carries (`sessions_per_subscription_max` in the acquisition module's
//! `numeric-provenance.json`, with his words); a lease goes to the usable
//! member carrying the fewest, and a pool whose every usable member is at
//! that number refuses the lease and is the shortage an acquisition buys for.

mod register;

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::gateway::broker;
use crate::subscription_dispatch::usage;

pub use register::{live, Lease};

/// Why no lease was given.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case", tag = "code")]
pub enum Refused {
    /// The caller named no provider, no session or no holder.
    Incomplete { detail: String },
    /// The provider has no declaration, or its declaration names no session
    /// limit the operator stated, so nothing can be shared out.
    LimitUnstated { detail: String },
    /// The pool holds no usable member of this provider: each is named with
    /// what the ledger holds against it.
    NoUsableMember {
        provider: String,
        members: Vec<Value>,
    },
    /// Every usable member carries the operator's limit of sessions.
    PoolFull {
        provider: String,
        limit: u64,
        members: Vec<Value>,
    },
    /// The register or the inventory could not be read or written.
    Unavailable { detail: String },
}

impl Refused {
    /// The sentence a refusal is reported with.
    pub fn detail(&self) -> String {
        match self {
            Self::Incomplete { detail } => detail.clone(),
            Self::LimitUnstated { detail } => format!("{detail}, so no lease can be shared out"),
            Self::NoUsableMember { provider, members } => format!(
                "the pool holds no usable {provider} member: {}",
                members
                    .iter()
                    .map(|member| format!("{} ({})", member["subscription_id"], member["refusal"]))
                    .collect::<Vec<_>>()
                    .join("; ")
            ),
            Self::PoolFull {
                provider,
                limit,
                members,
            } => format!(
                "every usable {provider} subscription carries {limit} sessions, the operator's limit: {}",
                members
                    .iter()
                    .map(|member| format!("{} carries {}", member["subscription_id"], member["live"]))
                    .collect::<Vec<_>>()
                    .join("; ")
            ),
            Self::Unavailable { detail } => detail.clone(),
        }
    }
}

/// What a caller gets for a lease: the lease itself and how many sessions the
/// subscription now carries, out of the operator's limit.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Taken {
    pub lease: Lease,
    pub live_on_subscription: u64,
    pub limit: u64,
    /// Whether this call made the lease, or found the session's standing one.
    pub new: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub provider: String,
    pub session_id: String,
    pub holder: String,
}

/// The usable members of `provider` and the live leases each carries, with
/// the members the ledger refuses set apart.
struct Members<'a> {
    usable: Vec<(&'a broker::SubscriptionEntry, Vec<Lease>)>,
    refused: Vec<Value>,
}

fn members<'a>(
    entries: &'a [broker::SubscriptionEntry],
    provider: &str,
    live: &[Lease],
) -> Members<'a> {
    let mut usable = Vec::new();
    let mut refused = Vec::new();
    for entry in entries
        .iter()
        .filter(|entry| entry.provider == provider && entry.status == "active")
        .filter(|entry| !crate::journal::is_retired(&entry.id))
    {
        match usage::standing_refusal(&entry.id) {
            Some(refusal) => refused.push(json!({
                "subscription_id": entry.id,
                "account": entry.account,
                "refusal": refusal.message,
            })),
            None => {
                let on_subscription: Vec<Lease> = live
                    .iter()
                    .filter(|lease| lease.subscription_id == entry.id)
                    .cloned()
                    .collect();
                usable.push((entry, on_subscription));
            }
        }
    }
    Members { usable, refused }
}

fn count(leases: &[Lease]) -> u64 {
    u64::try_from(leases.len()).expect("a lease count fits in u64")
}

fn member_row(entry: &broker::SubscriptionEntry, leases: &[Lease]) -> Value {
    json!({"subscription_id": entry.id, "account": entry.account, "live": count(leases)})
}

/// The order leases are given in: the fewest sessions first; among equals,
/// the freshest plan, as the rotation walk orders its candidates. A plan
/// nobody has read sorts before a read one, as an unknown fraction is not a
/// spent one; two fractions that cannot be compared (a reading that is not
/// a number) keep their listing order.
fn fewest_then_freshest(
    (left, left_live): &(&broker::SubscriptionEntry, Vec<Lease>),
    (right, right_live): &(&broker::SubscriptionEntry, Vec<Lease>),
) -> std::cmp::Ordering {
    left_live.len().cmp(&right_live.len()).then_with(|| {
        match usage::used_fraction(&left.id).partial_cmp(&usage::used_fraction(&right.id)) {
            Some(order) => order,
            None => std::cmp::Ordering::Equal,
        }
    })
}

/// Give `request.session_id` a lease on the usable member of its provider
/// carrying the fewest sessions, or refuse with the member counts. A session
/// that already holds a live lease on this provider gets that lease back:
/// a harness restarted by its own runtime is the same session.
pub async fn take(request: &Request) -> Result<Taken, Refused> {
    let provider = request.provider.trim();
    let session_id = request.session_id.trim();
    let holder = request.holder.trim();
    if provider.is_empty() || session_id.is_empty() || holder.is_empty() {
        return Err(Refused::Incomplete {
            detail: "a lease names its provider, the session_id that will run on it and the holder (the program that started the session)".to_string(),
        });
    }
    let limit = crate::subscription_dispatch::acquire::declaration::sessions_cap(provider)
        .map_err(|detail| Refused::LimitUnstated { detail })?;
    let entries = broker::list_all_subscriptions()
        .await
        .map_err(|error| Refused::Unavailable {
            detail: format!("the Skarbiec subscription inventory could not be read: {error}"),
        })?;
    let live = register::live().map_err(|detail| Refused::Unavailable { detail })?;
    if let Some(standing) = live
        .iter()
        .find(|lease| lease.session_id == session_id && lease.provider == provider)
    {
        let on_subscription: Vec<Lease> = live
            .iter()
            .filter(|lease| lease.subscription_id == standing.subscription_id)
            .cloned()
            .collect();
        return Ok(Taken {
            lease: standing.clone(),
            live_on_subscription: count(&on_subscription),
            limit,
            new: false,
        });
    }
    let Members {
        mut usable,
        refused,
    } = members(&entries, provider, &live);
    if usable.is_empty() {
        return Err(Refused::NoUsableMember {
            provider: provider.to_string(),
            members: refused,
        });
    }
    usable.sort_by(fewest_then_freshest);
    let Some((chosen, carried)) = usable.iter().find(|(_, live)| count(live) < limit) else {
        return Err(Refused::PoolFull {
            provider: provider.to_string(),
            limit,
            members: usable
                .iter()
                .map(|(entry, live)| member_row(entry, live))
                .collect(),
        });
    };
    let lease = register::take(
        provider,
        &chosen.id,
        chosen.account.as_deref(),
        session_id,
        holder,
    )
    .map_err(|detail| Refused::Unavailable { detail })?;
    let mut now_carried = carried.clone();
    now_carried.push(lease.clone());
    Ok(Taken {
        lease,
        live_on_subscription: count(&now_carried),
        limit,
        new: true,
    })
}

/// End the lease `lease_id`, or every live lease of `session_id`: the
/// session's terminal ended, so its subscription carries one session fewer.
/// Releasing what is not live is not an error: a release the caller repeats
/// changes nothing and says so.
pub fn release(lease_id: Option<&str>, session_id: Option<&str>) -> Result<Vec<Lease>, String> {
    register::release(lease_id, session_id)
}

/// How many live sessions each subscription carries, for the pool document
/// and the console: `subscription_id -> count`.
pub fn counts() -> Result<BTreeMap<String, u64>, String> {
    let mut carried: BTreeMap<String, Vec<Lease>> = BTreeMap::new();
    for lease in register::live()? {
        carried
            .entry(lease.subscription_id.clone())
            .or_default()
            .push(lease);
    }
    Ok(carried
        .into_iter()
        .map(|(subscription_id, leases)| (subscription_id, count(&leases)))
        .collect())
}

/// Whether every usable member of `provider` carries the operator's limit:
/// the shortage an acquisition buys for. `Ok(None)` when it is not.
pub async fn sessions_shortage(provider: &str) -> Result<Option<Value>, String> {
    let limit = crate::subscription_dispatch::acquire::declaration::sessions_cap(provider)?;
    let entries = broker::list_all_subscriptions().await.map_err(|error| {
        format!("the Skarbiec subscription inventory could not be read: {error}")
    })?;
    let live = register::live()?;
    let Members { usable, .. } = members(&entries, provider, &live);
    if usable.is_empty() {
        return Ok(None);
    }
    let full: Vec<Value> = usable
        .iter()
        .filter(|(_, live)| count(live) >= limit)
        .map(|(entry, live)| member_row(entry, live))
        .collect();
    if full.len() != usable.len() {
        return Ok(None);
    }
    Ok(Some(
        json!({"provider": provider, "limit": limit, "members": full}),
    ))
}
