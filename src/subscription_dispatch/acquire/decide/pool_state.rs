//! The two facts an acquisition is decided on: which accounts one provider's
//! pool holds, and whether every one of them has spent its plan right now.
//!
//! Both are read, never assumed. The accounts are the vault's own listing,
//! counted by the account each member declares, because one person's account
//! signed in twice is still one account. Whether an account is spent is the
//! provider's own usage report, read fresh for every member at the moment of
//! the decision (it costs a request and no quota), or a rate-limit block the
//! provider imposed that is still in force. An account whose plan cannot be
//! read is not a spent one: buying a new account because a grant broke would
//! pay for the wrong repair, so such a member stops the acquisition and is
//! named with its own refusal.

use std::collections::BTreeSet;

use crate::gateway::broker;
use crate::subscription_dispatch::{plan_usage, usage};

/// One pool member as the decision sees it.
#[derive(Clone, Debug)]
pub(super) struct Member {
    pub id: String,
    /// The account the member declares in `brama:account:`, lowercased.
    pub account: Option<String>,
}

/// Which account a member counts as.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum AccountKey {
    /// The account the member declares.
    Declared(String),
    /// A member that declares no account counts as an account of its own,
    /// named by its id, so the count can only err toward buying fewer.
    Undeclared(String),
}

impl std::fmt::Display for AccountKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Declared(account) => write!(f, "{account}"),
            Self::Undeclared(id) => write!(f, "{id} (declares no account)"),
        }
    }
}

/// Where one member stands at the moment of the decision.
#[derive(Clone, Debug)]
pub(super) enum Standing {
    /// The provider says this account has nothing left until `until_ms`.
    Spent { until_ms: Option<i64> },
    /// The account still has plan left in every current window; the most
    /// spent window's fraction, when the report carried one.
    Available { used_fraction: Option<f64> },
    /// The account's plan could not be read; the words are the refusal's.
    Unread { detail: String },
}

/// Every non-retired member of `provider`'s pool, as the vault lists it.
pub(super) async fn members(provider: &str) -> Result<Vec<Member>, String> {
    let entries = broker::list_all_subscriptions()
        .await
        .map_err(|error| format!("the Skarbiec subscription inventory could not be read: {error}"))?;
    Ok(entries
        .into_iter()
        .filter(|entry| entry.provider == provider && !crate::journal::is_retired(&entry.id))
        .map(|entry| Member {
            account: entry
                .account
                .as_deref()
                .map(str::trim)
                .filter(|account| !account.is_empty())
                .map(str::to_lowercase),
            id: entry.id,
        })
        .collect())
}

/// The distinct accounts the members belong to.
pub(super) fn accounts(members: &[Member]) -> BTreeSet<AccountKey> {
    members
        .iter()
        .map(|member| match &member.account {
            Some(account) => AccountKey::Declared(account.clone()),
            None => AccountKey::Undeclared(member.id.clone()),
        })
        .collect()
}

/// Whether a window is at its whole limit: a reading is a fraction of the
/// provider's own limit, so the whole limit is one.
fn at_limit(used_fraction: f64) -> bool {
    used_fraction >= 1.0 // https://brama.wisent.com/docs/concepts/subscription
}

/// Read one member's plan now and say where it stands.
pub(super) async fn standing(member: &Member, provider: &str) -> Standing {
    if let Some(until_ms) = usage::blocked_until_ms(&member.id) {
        return Standing::Spent {
            until_ms: Some(until_ms),
        };
    }
    if let Some(cause) = usage::awaiting_sign_in_cause(&member.id) {
        return Standing::Unread {
            detail: format!("its grant awaits a sign-in: {cause}"),
        };
    }
    if let Err(failure) = plan_usage::refresh(&member.id, provider).await {
        return Standing::Unread {
            detail: format!("its usage report could not be read: {failure}"),
        };
    }
    let recorded = usage::usage_for(&member.id);
    let windows = usage::plan_windows(recorded.as_ref());
    if windows.limits.is_empty() {
        return Standing::Unread {
            detail: "the provider's usage report named no plan window".to_string(),
        };
    }
    let now = chrono::Utc::now().timestamp_millis();
    let current: Vec<_> = windows
        .limits
        .iter()
        .filter(|reading| reading.resets_at_ms.is_none_or(|resets| resets > now))
        .collect();
    // One spent window is enough for the provider to refuse every request,
    // until the latest reset among the spent windows.
    let spent: Vec<_> = current
        .iter()
        .filter(|reading| at_limit(reading.used_fraction))
        .collect();
    if !spent.is_empty() {
        return Standing::Spent {
            until_ms: spent.iter().filter_map(|reading| reading.resets_at_ms).max(),
        };
    }
    Standing::Available {
        used_fraction: current
            .iter()
            .map(|reading| reading.used_fraction)
            .reduce(f64::max),
    }
}
