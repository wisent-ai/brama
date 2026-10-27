//! The pooled account a hand sign-in is for: an active member the pool
//! holds, or, on a standalone gateway, a new member its console names by
//! provider and account.
//!
//! A managed gateway banks a new member by donation, with a credential in
//! hand. A standalone gateway's launcher (Jeden's own gateway) has none
//! until it signs the account in, so there the sign-in itself banks the
//! member: it is added to the overlay just before the pasted code is
//! exchanged, so the grant is stored and proved against a member the pool
//! holds, and taken out again when the exchange fails.

use axum::http::StatusCode;

use crate::core::server::refusal::{api_error, ApiError};

/// A member the pool does not hold yet, as the console names it.
pub(in crate::core::server) struct Joining {
    pub(in crate::core::server) provider: String,
    pub(in crate::core::server) account: String,
}

/// The member a hand sign-in signs in.
pub(in crate::core::server) struct Target {
    pub(in crate::core::server) provider: String,
    pub(in crate::core::server) id: String,
    /// The account of a member this sign-in banks, and the console banking it.
    joining: Option<(String, String)>,
}

/// The active, unretired pooled account `subscription_id` names, or the
/// refusal that says why it cannot be signed in.
async fn active_account(
    subscription_id: &str,
) -> Result<crate::gateway::broker::SubscriptionEntry, ApiError> {
    let entries = crate::gateway::broker::list_all_subscriptions()
        .await
        .map_err(|detail| api_error(StatusCode::SERVICE_UNAVAILABLE, &detail))?;
    let entry = entries
        .into_iter()
        .find(|entry| entry.id == subscription_id)
        .ok_or_else(|| api_error(StatusCode::NOT_FOUND, "subscription not found"))?;
    let retired = crate::journal::is_retired(&entry.id)
        .map_err(|detail| api_error(StatusCode::SERVICE_UNAVAILABLE, &detail))?;
    if entry.status != "active" || retired {
        return Err(api_error(
            StatusCode::CONFLICT,
            "retired subscriptions cannot be signed in",
        ));
    }
    Ok(entry)
}

/// The member a hand sign-in with `reason` is for: the pooled account
/// `subscription_id` names, or the new member `joining` names, which only a
/// standalone gateway banks this way and only under an id no member holds.
pub(in crate::core::server) async fn target(
    subscription_id: &str,
    reason: &str,
    joining: Option<Joining>,
    console: &str,
) -> Result<Target, ApiError> {
    if reason.trim().is_empty() {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "--reason must say why this sign-in is being run",
        ));
    }
    let id = subscription_id.trim();
    if id.is_empty() {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "a subscription id is required",
        ));
    }
    let Some(joining) = joining else {
        let entry = active_account(id).await?;
        return Ok(Target {
            provider: entry.provider,
            id: entry.id,
            joining: None,
        });
    };
    if !crate::gateway::broker::local_provider_credentials_enabled() {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "a managed gateway banks a new member by donation with its credential; a hand sign-in here signs in a member the pool holds",
        ));
    }
    let account = joining.account.trim();
    let provider = joining.provider.trim();
    if account.is_empty() || provider.is_empty() {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "a new member needs a non-empty provider and account",
        ));
    }
    let held = crate::gateway::broker::list_all_subscriptions()
        .await
        .map_err(|detail| api_error(StatusCode::SERVICE_UNAVAILABLE, &detail))?;
    if held.iter().any(|entry| entry.id == id) {
        return Err(api_error(
            StatusCode::CONFLICT,
            &format!("subscription {id} is already in the pool; sign it in without a provider and an account"),
        ));
    }
    Ok(Target {
        provider: provider.to_owned(),
        id: id.to_owned(),
        joining: Some((account.to_owned(), console.to_owned())),
    })
}

/// A member this sign-in added to the pool: only it is taken out again when
/// the exchange fails, never a member a donation or another sign-in banked
/// under the same id.
pub(in crate::core::server) struct Banked(String);

impl Banked {
    /// Take the member out of the overlay again, after the exchange that
    /// would have given it a grant failed.
    pub(in crate::core::server) fn unbank(self) -> Result<(), String> {
        crate::gateway::broker::donated_remove(&self.0)
    }
}

impl Target {
    /// Add the member this sign-in banks to the overlay, once the pasted
    /// code is in hand; nothing for a member the pool already held when the
    /// sign-in began. The id is checked free again here, because a donation
    /// or another sign-in may have banked it while the page was open.
    pub(in crate::core::server) async fn bank(&self) -> Result<Option<Banked>, ApiError> {
        let Some((account, console)) = &self.joining else {
            return Ok(None);
        };
        let held = crate::gateway::broker::list_all_subscriptions()
            .await
            .map_err(|detail| api_error(StatusCode::SERVICE_UNAVAILABLE, &detail))?;
        if held.iter().any(|entry| entry.id == self.id) {
            return Err(api_error(
                StatusCode::CONFLICT,
                &format!(
                    "subscription {} joined the pool while this sign-in's page was open, by a donation or another sign-in; this sign-in does not bank it again: sign it in without a provider and an account",
                    self.id
                ),
            ));
        }
        crate::gateway::broker::donated_add(console, &self.id, &self.provider, Some(account), None)
            .map_err(|detail| api_error(StatusCode::SERVICE_UNAVAILABLE, &detail))?;
        Ok(Some(Banked(self.id.clone())))
    }
}
