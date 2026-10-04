//! Is this process up, and can it carry traffic?
//!
//! Two different questions with two different costs. `/health` answers the
//! first from nothing at all. The second crosses the Skarbiec broker and
//! provider discovery, so [`check`] runs it once when the gateway starts and
//! again on every `brama maintain` pass, and `/readyz` returns the last
//! completed answer.

mod accounts;
pub(in crate::core::server) mod check;
mod placement;

use std::sync::LazyLock;

use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use serde_json::{json, Value};

pub(in crate::core::server) async fn health() -> impl IntoResponse {
    Json(json!({
        "status": "ok",
        "build": crate::build_info::current(),
        "dependencies": "not_probed",
    }))
}

#[derive(Clone)]
pub(super) struct ReadinessReport {
    status: StatusCode,
    body: Value,
}

impl ReadinessReport {
    fn pending() -> Self {
        Self {
            status: StatusCode::SERVICE_UNAVAILABLE,
            body: json!({
                "ready": false,
                "reason": "readiness check has not completed",
                "degraded": true,
                "providers": [],
                "denied": [],
                "routing": [],
                "unroutable": [],
                "subscriptions": [],
                "unredeemable": [],
                "unroutable_accounts": [],
                "operator_action_required": false,
                "build": crate::build_info::current(),
            }),
        }
    }

    /// The first half of one sweep: the direct-provider capabilities, before
    /// any subscription has been discovered or redeemed. `ready` when one of
    /// them was obtained — that gateway carries traffic — and `degraded`
    /// either way, because the subscription half is still unknown; the
    /// reason says so, so nobody reads this as the whole verdict.
    pub(super) fn interim(
        provider_available: bool,
        checked: Vec<Value>,
        denied: Vec<String>,
    ) -> Self {
        Self {
            status: if provider_available {
                StatusCode::OK
            } else {
                StatusCode::SERVICE_UNAVAILABLE
            },
            body: json!({
                "ready": provider_available,
                "reason": if provider_available {
                    "a direct provider credential was obtained; the subscription sweep is still running"
                } else {
                    "no direct provider credential was obtained; the subscription sweep is still running"
                },
                "degraded": true,
                "providers": checked,
                "denied": denied,
                "routing": [],
                "unroutable": [],
                "subscriptions": [],
                "unredeemable": [],
                "unroutable_accounts": [],
                "operator_action_required": false,
                "build": crate::build_info::current(),
            }),
        }
    }
}

// A std lock, not tokio's: the sweep publishes its interim verdict from
// inside a synchronous callback, and a reader holds this for one clone.
static READINESS_REPORT: LazyLock<std::sync::RwLock<ReadinessReport>> =
    LazyLock::new(|| std::sync::RwLock::new(ReadinessReport::pending()));

fn publish(report: ReadinessReport) {
    match READINESS_REPORT.write() {
        Ok(mut current) => *current = report,
        Err(poisoned) => *poisoned.into_inner() = report,
    }
}

/// The last `brama maintain` pass this process ran: when it finished, in Unix
/// seconds, and whether every step succeeded. `None` until the first pass.
static LAST_MAINTENANCE: LazyLock<std::sync::RwLock<Option<(u64, bool)>>> =
    LazyLock::new(|| std::sync::RwLock::new(None));

/// Record one finished maintenance pass for `/readyz`.
pub(in crate::core::server) fn record_maintenance(ok: bool) {
    let finished = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or_default();
    match LAST_MAINTENANCE.write() {
        Ok(mut last) => *last = Some((finished, ok)),
        Err(poisoned) => *poisoned.into_inner() = Some((finished, ok)),
    }
}

/// What `/readyz` says about maintenance. The gateway keeps no timer, so it
/// cannot tell a late pass from an early one; it states when the last pass
/// ended and how, and says plainly when none has run, because then nothing
/// renews a grant or schedules a sign-in until the host's Stado schedule runs
/// `brama maintain`.
fn maintenance_state() -> Value {
    let last = match LAST_MAINTENANCE.read() {
        Ok(last) => *last,
        Err(poisoned) => *poisoned.into_inner(),
    };
    match last {
        Some((finished, ok)) => json!({"last_pass_unix": finished, "last_pass_ok": ok}),
        None => json!({
            "last_pass_unix": null,
            "detail": "no maintenance pass has run since this process started: nothing renews a \
                       grant or schedules a sign-in until the host's Stado schedule runs brama maintain"
        }),
    }
}

/// Return the last completed credential and routing check, with the last
/// maintenance pass beside it.
pub(in crate::core::server) async fn readyz() -> impl IntoResponse {
    let report = match READINESS_REPORT.read() {
        Ok(current) => current.clone(),
        Err(poisoned) => poisoned.into_inner().clone(),
    };
    let mut body = report.body;
    if let Some(object) = body.as_object_mut() {
        object.insert("maintenance".to_string(), maintenance_state());
    }
    (report.status, Json(body))
}

/// Learn this host's placement and take the first readiness reading, once.
pub(in crate::core::server) fn spawn_readiness_probe() {
    // Off the readiness path on purpose: this shells out to Stado.
    placement::learn();
    tokio::spawn(recompute());
}

/// Take one readiness reading, publish it for `/readyz`, and return its body.
pub(in crate::core::server) async fn recompute() -> Value {
    let report = check::calculate_readiness(publish).await;
    let body = report.body.clone();
    publish(report);
    body
}
