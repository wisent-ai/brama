//! What each real account has left, stated per account rather than per
//! member.
//!
//! The ledger records per member, and `accounts` says which accounts the
//! members belong to. Whoever decides whether another account is bought, or
//! on which account a session a provider limit stopped is put next, needs
//! the provider's own statement per account: what fraction of each plan
//! window (the five-hour and the weekly ones) is gone, when it resets, and
//! whether any member of the account can be presented at all. Two members
//! of one account draw on one quota, so the newest reading of each window
//! across them is the account's reading.
//!
//! Nothing here is a number the provider did not state. What is left of a
//! window is the whole limit less the provider's used fraction; a window
//! whose reset instant has passed is served as not current, with the
//! reading it last had, rather than as a free window; an account with no
//! reading at all says so instead of reporting a free plan.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::gateway::broker::SubscriptionEntry;
use crate::subscription_dispatch::acquire::{at_limit, WHOLE_LIMIT};
use crate::subscription_dispatch::usage::{self, SubscriptionUsage};

/// The newest reading of one plan window across an account's members.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Window {
    pub limit_id: String,
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub window_label: Option<String>,
    /// The provider's statement of how much of the window is gone.
    pub used_fraction: f64,
    /// The whole limit less `used_fraction`; `null` for a window that is not
    /// current, because the provider's clock says it rolled and the reading
    /// no longer describes it.
    pub remaining_fraction: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resets_at_ms: Option<i64>,
    /// When Brama read this window, and off which member.
    pub read_at_ms: i64,
    pub member: String,
    /// Whether the reset instant is still ahead (a window that states none is
    /// current until a newer reading replaces it).
    pub current: bool,
}

/// One member the account cannot be presented on, in the ledger's words.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RefusedMember {
    pub member: String,
    pub refusal: String,
}

/// What one account has left and on which members.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Account {
    pub provider: String,
    pub account: String,
    pub members: Vec<String>,
    /// The members nothing in the ledger stands against.
    pub usable_members: Vec<String>,
    pub refused_members: Vec<RefusedMember>,
    /// The latest instant a standing block on any member lifts.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blocked_until_ms: Option<i64>,
    /// The live sessions all members carry; `null` when the lease register
    /// could not be read.
    pub live_sessions: Option<u64>,
    pub windows: Vec<Window>,
    /// The least that is left across the current windows; `null` when no
    /// current window was read.
    pub tightest_remaining_fraction: Option<f64>,
    /// Whether a session put on this account now would be served: a usable
    /// member exists and no current window is at its limit.
    pub available: bool,
}

/// One member as the pool document observed it while building its rows.
pub(crate) struct Observed<'a> {
    pub entry: &'a SubscriptionEntry,
    pub recorded: Option<SubscriptionUsage>,
    pub retired: bool,
    pub live_sessions: Option<u64>,
}

/// The pool's accounts with what each has left, from the members the
/// document observed. Members that are not active, are retired or record no
/// account belong to no account here; `accounts` already names them.
pub(crate) fn accounts(observed: &[Observed<'_>], now_ms: i64) -> Vec<Account> {
    let mut grouped: BTreeMap<(String, String), Vec<&Observed<'_>>> = BTreeMap::new();
    for member in observed {
        if member.entry.status != "active" || member.retired {
            continue;
        }
        let Some(account) = member
            .entry
            .account
            .as_deref()
            .map(str::trim)
            .filter(|account| !account.is_empty())
        else {
            continue;
        };
        grouped
            .entry((member.entry.provider.clone(), account.to_lowercase()))
            .or_default()
            .push(member);
    }
    grouped
        .into_iter()
        .map(|((provider, account), members)| summarize(provider, account, &members, now_ms))
        .collect()
}

fn summarize(provider: String, account: String, members: &[&Observed<'_>], now_ms: i64) -> Account {
    let mut usable_members = Vec::new();
    let mut refused_members = Vec::new();
    let mut blocked_until_ms: Option<i64> = None;
    let mut newest: BTreeMap<String, Window> = BTreeMap::new();
    for member in members {
        let id = &member.entry.id;
        match usage::standing_refusal(id) {
            Some(refusal) => refused_members.push(RefusedMember {
                member: id.clone(),
                refusal: refusal.message,
            }),
            None => usable_members.push(id.clone()),
        }
        if let Some(until) = usage::blocked_until_ms(id) {
            blocked_until_ms = Some(blocked_until_ms.map_or(until, |held| held.max(until)));
        }
        for reading in usage::plan_windows(member.recorded.as_ref()).limits {
            let current = reading.resets_at_ms.is_none_or(|resets| resets > now_ms);
            let window = Window {
                remaining_fraction: current.then(|| WHOLE_LIMIT - reading.used_fraction),
                limit_id: reading.limit_id.clone(),
                label: reading.label,
                window_label: reading.window_label,
                used_fraction: reading.used_fraction,
                resets_at_ms: reading.resets_at_ms,
                read_at_ms: reading.recorded_at_ms,
                member: id.clone(),
                current,
            };
            match newest.get(&reading.limit_id) {
                Some(held) if held.read_at_ms >= window.read_at_ms => {}
                _ => {
                    newest.insert(reading.limit_id, window);
                }
            }
        }
    }
    // A register that could not be read for one member was not read for any:
    // the sum is then unknown rather than short.
    let live_sessions = members
        .iter()
        .map(|member| member.live_sessions)
        .sum::<Option<u64>>();
    let windows: Vec<Window> = newest.into_values().collect();
    let tightest_remaining_fraction = windows
        .iter()
        .filter_map(|window| window.remaining_fraction)
        .reduce(f64::min);
    let available = !usable_members.is_empty()
        && !windows
            .iter()
            .any(|window| window.current && at_limit(window.used_fraction));
    Account {
        provider,
        account,
        members: members
            .iter()
            .map(|member| member.entry.id.clone())
            .collect(),
        usable_members,
        refused_members,
        blocked_until_ms,
        live_sessions,
        windows,
        tightest_remaining_fraction,
        available,
    }
}
