//! The serving process's own maintenance passes, and the refusal that wakes
//! one.
//!
//! A pass reads every plan usage report that is due, renews every grant
//! whose stated expiry has passed, signs accounts in through Weles, decides
//! whether a spent pool needs a new account, and takes a fresh readiness
//! reading. The serving process runs one when it starts, then one at the next
//! moment the ledger states something becomes due (a grant's stated expiry, a
//! plan window's stated reset, a rate block's stated end), and one whenever a
//! request is refused for want of a signed-in subscription. No host schedule
//! has to fire for a grant to be renewed or a member to be signed in, and no
//! clock of Brama's decides when: `brama maintain` is one more pass beside
//! these, for an operator who wants one now.

use std::sync::LazyLock;
use std::time::Duration;

use serde_json::{json, Value};
use tokio::sync::Notify;
use tracing::info;

use crate::subscription_dispatch::discovery::harness::DiscoveryReport;

/// Wakes the serving process's maintenance before its next stated moment. A
/// wake while a pass runs is kept, so the pass after it starts at once.
static WAKE: LazyLock<Notify> = LazyLock::new(Notify::new);

/// Run one maintenance pass, with the account discovery report `supplied`
/// when the caller observed one and the harness's own otherwise. `ok` is
/// false when any step failed, and each failure is in the report with its
/// error; an acquisition refusal is a decision, not a failure.
pub(in crate::core::server) async fn pass(supplied: Option<DiscoveryReport>) -> Value {
    let observed = match supplied {
        Some(report) => report,
        None => crate::subscription_dispatch::discovery::harness::collect().await,
    };
    let discovery = crate::subscription_dispatch::discovery::enroll(observed).await;
    let plan_usage = crate::subscription_dispatch::plan_usage::sweep().await;
    // A member imported without its account cannot be resolved for a sign-in;
    // its own grant usually states the account, so it is recorded first.
    let attribution = attribute_accounts().await;
    let credentials = crate::subscription_dispatch::refresh_sweep::sweep().await;
    let resets = crate::subscription_dispatch::acquire::resets::maintenance::run().await;
    let acquisition = if discovery["ok"].as_bool() == Some(true)
        && resets["ok"].as_bool() == Some(true)
    {
        crate::subscription_dispatch::acquire::maintenance_pass().await
    } else {
        json!({"verdicts": [], "result": "refused",
            "detail": "account discovery or saved-capacity maintenance is incomplete; no account was purchased"})
    };
    let readiness = crate::core::server::readiness::recompute().await;
    let usage_failed = plan_usage
        .get("failed")
        .and_then(Value::as_array)
        .is_some_and(|failed| !failed.is_empty());
    let ready = readiness.get("ready").and_then(Value::as_bool) == Some(true);
    let ok = discovery["ok"].as_bool() == Some(true)
        && attribution["ok"].as_bool() == Some(true)
        && resets["ok"].as_bool() == Some(true)
        && !usage_failed
        && credentials.is_ok()
        && ready;
    crate::core::server::readiness::record_maintenance(ok);
    json!({
        "ok": ok,
        "discovery": discovery,
        "plan_usage": plan_usage,
        "attribution": attribution,
        "resets": resets,
        "credentials": match credentials {
            Ok(report) => report,
            Err(error) => json!({"error": error}),
        },
        "acquisition": acquisition,
        "readiness": readiness,
    })
}

/// Record, for every provider the vault lists an active renewable member of,
/// the account each member's own grant states
/// ([`crate::subscription_dispatch::pool::record_accounts`]). A member whose
/// grant states none stays unattributed and is reported with the reason; only
/// an inventory or provider pass that could not be read is a failure.
async fn attribute_accounts() -> Value {
    let entries = match crate::gateway::broker::list_all_subscriptions().await {
        Ok(entries) => entries,
        Err(error) => {
            return json!({"ok": false,
                "error": format!("the Skarbiec subscription inventory could not be read: {error}")})
        }
    };
    let providers: std::collections::BTreeSet<String> = entries
        .into_iter()
        .filter(|entry| {
            entry.status == "active"
                && crate::gateway::broker::supports_oauth_refresh(&entry.provider)
        })
        .map(|entry| entry.provider)
        .collect();
    let mut reports = serde_json::Map::new();
    let mut failed = Vec::new();
    for provider in providers {
        match crate::subscription_dispatch::pool::record_accounts(&provider).await {
            Ok(report) => reports.insert(provider, report),
            Err(error) => {
                failed.push(provider.clone());
                reports.insert(provider, json!({"error": error}))
            }
        };
    }
    json!({"ok": failed.is_empty(), "failed": failed, "providers": reports})
}

/// Run the serving process's maintenance: a pass now, then one at each next
/// moment the ledger states, or sooner when woken.
pub(in crate::core::server) fn spawn() {
    tokio::spawn(async {
        loop {
            let woken = WAKE.notified();
            tokio::pin!(woken);
            woken.as_mut().enable();
            let report = pass(None).await;
            let due = crate::subscription_dispatch::usage::next_due_ms();
            info!(
                event = "maintenance_pass_finished",
                ok = report["ok"].as_bool() == Some(true),
                next_due_ms = due,
                "the serving process's maintenance pass finished"
            );
            let remaining = due.map(|at| at.saturating_sub(chrono::Utc::now().timestamp_millis()));
            match remaining.filter(|left| left.is_positive()) {
                Some(left) => {
                    let moment = tokio::time::Instant::now() + Duration::from_millis(left.unsigned_abs());
                    tokio::select! {
                        () = &mut woken => {}
                        () = tokio::time::sleep_until(moment) => {}
                    }
                }
                // A moment that passed while the pass ran is due now; with
                // none stated, the next refusal or `brama maintain` wakes it.
                None if due.is_some() => {}
                None => woken.await,
            }
        }
    });
}

/// Wake the serving process's maintenance because a request was just
/// refused for want of a signed-in subscription. The refusal is answered at
/// once; the pass runs beside it, or right after the one already running.
pub(in crate::core::server) fn start_after_refusal() {
    info!(
        event = "maintenance_woken_by_refusal",
        "a request was refused for want of a signed-in subscription; a maintenance pass is due"
    );
    WAKE.notify_one();
}
