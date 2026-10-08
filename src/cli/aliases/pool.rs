//! What the pool holds, for the aliases this gateway cannot check on its own.

use serde_json::Value;

/// `serving` for a subscription selector means the selector is declared. It
/// says nothing about a credential: `best serving best` can be printed while
/// every request for `best` is refused with
/// `subscription_reauthorization_required`, because the pool holds no live
/// member. The pool's own count stands beside it.
pub(super) struct PoolCount {
    pub(super) live: usize,
    pub(super) members: usize,
}

pub(super) async fn pool_count() -> PoolCount {
    let scope = brama::subscription_dispatch::pool::PoolScope::Deployment;
    let report = brama::subscription_dispatch::pool::report(&scope).await;
    let rows = report.get("subscriptions").and_then(Value::as_array);
    PoolCount {
        live: rows
            .into_iter()
            .flatten()
            .filter(|row| row.get("state").and_then(Value::as_str) == Some("live"))
            .count(),
        members: rows.into_iter().flatten().count(),
    }
}
