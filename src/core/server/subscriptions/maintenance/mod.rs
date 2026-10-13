//! One maintenance pass of this serving process, and the refusal that starts
//! one.
//!
//! A pass reads every plan usage report that is due, renews every grant
//! whose stated expiry has passed, signs accounts in through Weles, decides
//! whether a spent pool needs a new account, and takes a fresh readiness
//! reading. The host's Stado schedule calls it through `brama maintain`.
//! That schedule runs through the fleet store, and a fleet store whose vault
//! stops answering fires nothing; a gateway that waited for it would refuse
//! every request `subscription_reauthorization_required` while nothing
//! renewed a grant. So the refusal itself starts a pass: a request refused
//! for want of a signed-in subscription starts one, unless one is already
//! running, and the gateway repairs itself on the next request instead of
//! waiting for a schedule that may never fire. No clock decides it.

use std::sync::atomic::{AtomicBool, Ordering};

use serde_json::{json, Value};
use tracing::info;

use crate::subscription_dispatch::discovery::harness::DiscoveryReport;

/// Whether a pass a refusal started is running now.
static REFUSAL_PASS: AtomicBool = AtomicBool::new(false);

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
        && resets["ok"].as_bool() == Some(true)
        && !usage_failed
        && credentials.is_ok()
        && ready;
    crate::core::server::readiness::record_maintenance(ok);
    json!({
        "ok": ok,
        "discovery": discovery,
        "plan_usage": plan_usage,
        "resets": resets,
        "credentials": match credentials {
            Ok(report) => report,
            Err(error) => json!({"error": error}),
        },
        "acquisition": acquisition,
        "readiness": readiness,
    })
}

/// Start one maintenance pass because a request was just refused for want
/// of a signed-in subscription, unless a pass a refusal started is still
/// running. The pass runs beside the refusal, which is answered at once.
pub(in crate::core::server) fn start_after_refusal() {
    if REFUSAL_PASS.swap(true, Ordering::AcqRel) {
        return;
    }
    info!(
        event = "maintenance_started_by_refusal",
        "a request was refused for want of a signed-in subscription; one maintenance pass starts"
    );
    tokio::spawn(async {
        let report = pass(None).await;
        info!(
            event = "maintenance_by_refusal_finished",
            ok = report["ok"].as_bool() == Some(true),
            "the maintenance pass a refusal started finished"
        );
        REFUSAL_PASS.store(false, Ordering::Release);
    });
}
