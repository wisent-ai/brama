//! Replace an access token before it dies, instead of learning it is dead from
//! a refused request.
//!
//! Brama used to refresh a grant at exactly one moment: inside a request, after
//! the local expiry said the token was spent or after the provider rejected it.
//! That is late in two ways. The request pays for the refresh, and -- worse --
//! nothing at all happens for a subscription no request reaches, so a credential
//! whose refresh token the provider had already disowned kept sitting in the
//! vault reading as active. Four of them did, for five days, while every request
//! that touched them failed and nothing said why.
//!
//! This module is the other moment: a short timer that walks every active
//! subscription, refreshes each grant that expires inside a skew window, and
//! records what the provider said about the ones it refuses. Four rules hold
//! here.
//!
//! A refresh is single-flighted per subscription, so a slow one is never
//! started twice; the rotation lock in the broker already serialises the writes,
//! and this is what stops a sweep from queueing behind its own previous attempt.
//! One subscription's failure -- including a panic -- ends that subscription's
//! turn and nothing else. A credential the provider has definitively refused is
//! left alone until a sign-in replaces it, because asking again every minute
//! cannot produce a different answer and would bury the one log line that
//! matters. And the ledger is consulted before the vault is: reading a
//! credential shells out to the entitlements router, so a grant whose recorded
//! expiry is hours away is skipped without reading anything at all.
//!
//! Nothing here spends provider quota: a token endpoint is not a metered
//! endpoint, so this sweep can run on a short timer without costing an account
//! anything it would otherwise have spent on a request.

mod cadence;
mod claim;
mod reauthorization;
mod renewal;
mod verdict;

use std::collections::BTreeSet;
use std::time::Duration;

use tracing::info;

use crate::gateway::broker;
use crate::subscription_dispatch::usage::{self, RefreshHint};

use reauthorization::schedule_sign_in;
use renewal::{refresh_one, Swept};

pub use cadence::spawn;

/// Browser sign-in is separate from silent OAuth token renewal.
const AUTOMATIC_SIGN_IN_ENV: &str = "BRAMA_CREDENTIAL_AUTOMATIC_SIGN_IN";

/// Walk every active subscription once, refreshing what is due.
async fn sweep(skew: Duration) {
    let automatic_sign_in = std::env::var(AUTOMATIC_SIGN_IN_ENV).is_ok_and(|value| value == "1");
    let mut visited = BTreeSet::new();
    let mut refreshed = usize::default();
    let mut refused = usize::default();
    let mut awaiting_signin = usize::default();
    let mut sign_ins_started = usize::default();
    let mut entries = Vec::new();
    for agent in broker::configured_request_sign_agents() {
        entries.extend(broker::list_subscriptions(&agent).await);
    }
    // Incomplete historical items are never handed to request dispatch. They
    // enter only this repair loop; the Weles account declaration must map back
    // to the exact subscription id before a browser opens.
    if automatic_sign_in {
        entries.extend(broker::list_recoverable_subscriptions().await);
    }
    for entry in entries {
        if entry.status != "active" || crate::journal::is_retired(&entry.id) {
            continue;
        }
        // Only a provider whose credentials are OAuth grants has anything to
        // do here. An API key has no access token that expires, so reading
        // one to discover that would cost a vault read every minute and
        // learn nothing.
        if !broker::supports_oauth_refresh(&entry.provider) {
            continue;
        }
        // A subscription two agents share is one account with one grant, and
        // refreshing it twice would rotate a refresh token the first pass
        // has already replaced.
        if !visited.insert(entry.id.clone()) {
            continue;
        }
        // A historical OAuth item without its exact Weles account is not fully
        // owned, even while its current access token still works. Repair that
        // identity now instead of waiting for an expiry or provider refusal:
        // the requested subscription id lets Weles accept only its declared
        // account, and a successful donation writes the durable login tag.
        if automatic_sign_in
            && entry
                .login_item
                .as_deref()
                .map(str::trim)
                .filter(|item| !item.is_empty())
                .is_none()
        {
            awaiting_signin = awaiting_signin.saturating_add(1);
            if schedule_sign_in(entry.id, entry.provider, entry.login_item) {
                sign_ins_started = sign_ins_started.saturating_add(1);
            }
            continue;
        }
        // What the ledger already knows decides whether the vault is read at
        // all. A grant the provider has disowned is not retried until a
        // sign-in replaces it, because the answer cannot change and asking
        // every minute would cost one log line a minute per dead account. A
        // grant whose recorded expiry is hours away is not read either: the
        // read shells out to the entitlements router, and this file already
        // holds the number that read would return.
        match usage::credential_refresh_hint(&entry.id, skew) {
            RefreshHint::AwaitingSignIn => {
                awaiting_signin = awaiting_signin.saturating_add(1);
                if automatic_sign_in && schedule_sign_in(entry.id, entry.provider, entry.login_item)
                {
                    sign_ins_started = sign_ins_started.saturating_add(1);
                }
                continue;
            }
            RefreshHint::NotDue => continue,
            RefreshHint::Read => {}
        }
        let login_item = entry.login_item;
        match refresh_one(entry.id, entry.provider, skew).await {
            Swept::Refreshed => refreshed = refreshed.saturating_add(1),
            Swept::Refused => refused = refused.saturating_add(1),
            Swept::AwaitingSignIn {
                subscription_id,
                provider,
            } => {
                awaiting_signin = awaiting_signin.saturating_add(1);
                if automatic_sign_in && schedule_sign_in(subscription_id, provider, login_item) {
                    sign_ins_started = sign_ins_started.saturating_add(1);
                }
            }
            Swept::NotDue | Swept::Skipped => {}
        }
    }
    info!(
        event = "credential_refresh_sweep_finished",
        subscriptions = visited.len(),
        refreshed,
        refused,
        awaiting_signin,
        sign_ins_started,
        automatic_sign_in,
        "finished one credential refresh sweep"
    );
}
