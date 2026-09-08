//! What the ledger alone can tell a renewal sweep, before anything reads the
//! vault.
//!
//! Reading a credential means shelling out to the entitlements router, and
//! doing that once a minute per subscription to re-read an expiry this ledger
//! already holds is the sort of cost that gets a background task switched off.
//! So the sweep asks here first, and only the subscriptions this cannot rule
//! out are read for real.
//!
//! Two answers are given from the same record and neither treats silence as
//! evidence: whether a refresh could be due inside a window, and when the
//! ledger's verdict about the grant was recorded -- which is the instant a
//! repair loop compares itself against to know whether another sign-in could
//! possibly answer differently.

use std::time::Duration;

use crate::subscription_dispatch::usage::{now_ms, read_ledger};

/// What the ledger alone says about one subscription's grant, before anything is
/// read from the vault.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RefreshHint {
    /// The provider disowned this grant, or it was retired. Leave it alone: no
    /// refresh can repair it and only a sign-in replaces it.
    AwaitingSignIn,
    /// The recorded expiry is further away than the window asked about, so
    /// nothing is due.
    NotDue,
    /// Nothing recorded rules a refresh out. Read the credential and decide from
    /// what it says.
    Read,
}

/// What the ledger already knows about whether this grant needs refreshing
/// within `within`.
///
/// One ledger read answers both questions a sweep asks, because each of them
/// otherwise costs a load of its own. And answering them from here at all is
/// what keeps a sweep cheap: reading the credential means shelling out to the
/// entitlements router, and doing that once a minute per subscription to
/// re-read an expiry this file already holds is the sort of cost that gets a
/// background task switched off.
///
/// Silence is never evidence. A subscription with no recorded credential
/// answers [`RefreshHint::Read`], so a host whose ledger file was just created
/// refreshes normally instead of skipping every account on it.
pub fn credential_refresh_hint(subscription_id: &str, within: Duration) -> RefreshHint {
    let horizon = now_ms().saturating_add(i64::try_from(within.as_millis()).unwrap_or(i64::MAX));
    read_ledger(|ledger| {
        let Some(credential) = ledger
            .subscriptions
            .get(subscription_id)
            .and_then(|entry| entry.credential.as_ref())
        else {
            return RefreshHint::Read;
        };
        if !credential.state.usable() {
            return RefreshHint::AwaitingSignIn;
        }
        match credential.expires_at_ms {
            // A recorded expiry beyond the horizon is this credential's own
            // statement about itself, written the last time it was read or
            // refreshed. A grant replaced outside Brama can make it wrong, and
            // the request path's own refresh is what covers that case.
            Some(expires_at_ms) if expires_at_ms > horizon => RefreshHint::NotDue,
            _ => RefreshHint::Read,
        }
    })
}

/// When the ledger's verdict about one subscription's grant was recorded, or
/// `None` when nothing has ever been recorded about it.
///
/// This is the instant a repair loop compares itself against. A browser
/// sign-in can only change the answer if the stored credential changed, and the
/// ledger writes a new instant every time it does -- a refusal, a rotation, a
/// sign-in that stored something. A verdict no newer than the last sign-in
/// therefore proves that sign-in has already been tried against exactly this
/// state and produced this.
pub fn credential_recorded_at_ms(subscription_id: &str) -> Option<i64> {
    read_ledger(|ledger| {
        ledger
            .subscriptions
            .get(subscription_id)
            .and_then(|entry| entry.credential.as_ref())
            .map(|credential| credential.recorded_at_ms)
    })
}
