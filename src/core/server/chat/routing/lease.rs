//! The leased subscription a session names in its request headers, read as
//! the billing target the call is charged to.

use axum::http::HeaderMap;

use crate::types::BillingTarget;

/// The header a session on a leased subscription sends with every request.
pub(crate) const SUBSCRIPTION_HEADER: &str = "x-brama-subscription";
/// The account the leased member declares, when it declares one.
pub(crate) const ACCOUNT_HEADER: &str = "x-brama-account";

/// The billing target the request headers name, `None` when they name none,
/// refused by name when the header is not `<provider>:<subscription_id>`.
/// The account is the member's declared one, or its id when it declares
/// none, as the rotation counts members.
pub(super) fn leased_target(headers: &HeaderMap) -> Result<Option<BillingTarget>, String> {
    let Some(named) = headers.get(SUBSCRIPTION_HEADER) else {
        return Ok(None);
    };
    let named = named.to_str().map_err(|_| {
        format!("{SUBSCRIPTION_HEADER} is not text; it names <provider>:<subscription_id>")
    })?;
    let (provider, subscription) = named
        .split_once(':')
        .map(|(provider, subscription)| (provider.trim(), subscription.trim()))
        .filter(|(provider, subscription)| !provider.is_empty() && !subscription.is_empty())
        .ok_or_else(|| {
            format!("{SUBSCRIPTION_HEADER} is {named:?}; it names <provider>:<subscription_id>")
        })?;
    let account = match headers.get(ACCOUNT_HEADER) {
        Some(account) => account
            .to_str()
            .map_err(|_| format!("{ACCOUNT_HEADER} is not text; it names the member's account"))?
            .trim()
            .to_string(),
        None => String::new(),
    };
    Ok(Some(BillingTarget {
        provider_id: provider.to_string(),
        account_id: if account.is_empty() {
            subscription.to_string()
        } else {
            account
        },
        subscription_id: subscription.to_string(),
    }))
}
