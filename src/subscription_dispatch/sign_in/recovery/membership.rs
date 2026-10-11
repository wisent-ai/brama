//! Admitting a recoverable account to a sign-in. A vault item that names an
//! exact account (`brama:id:`, `brama:provider:`, `brama:account:`) but lost
//! its `brama:subscription` mark is outside the pool: discovery does not list
//! it, the renewal sweep never schedules its sign-in, Weles's login resolution
//! does not see it, and a sign-in asked for it by id answered "subscription
//! not found". Readiness reported it as unroutable and left the repair to
//! somebody's hand. The account is real and its identity is complete, so the
//! mark is restored and the sign-in goes on: the independent grant Weles
//! banks on it is what makes it routable, exactly as a discovered account.

use crate::gateway::broker::{self, SubscriptionEntry};

/// The pool member `subscription_id` names once a recoverable account with
/// that id has had its routing mark restored; `None` when no recoverable
/// account has that id. An account that names no provider account is refused
/// by name: without one there is no login to resolve a sign-in from.
pub async fn admit_recoverable(subscription_id: &str) -> Result<Option<SubscriptionEntry>, String> {
    let recoverable = broker::list_unroutable_accounts().await?;
    let Some(account) = recoverable
        .into_iter()
        .find(|account| account.id.as_deref() == Some(subscription_id))
    else {
        return Ok(None);
    };
    let provider = account.provider.clone().ok_or_else(|| {
        format!(
            "subscription {subscription_id}: vault item {} carries no brama:provider: tag, so it cannot be signed in",
            account.item
        )
    })?;
    let identity = account.account.clone().ok_or_else(|| {
        format!(
            "subscription {subscription_id}: vault item {} names no provider account (brama:account:), so no login can be resolved for it; record the account on the item and sign in again",
            account.item
        )
    })?;
    broker::restore_subscription_mark(&provider, subscription_id, &account.item, &identity).await?;
    let admitted = broker::list_all_subscriptions()
        .await?
        .into_iter()
        .find(|entry| entry.id == subscription_id)
        .ok_or_else(|| {
            format!(
                "subscription {subscription_id}: its routing mark was restored on {}, but the pool still does not list it",
                account.item
            )
        })?;
    Ok(Some(admitted))
}
