//! Is this process up, and can it carry traffic?
//!
//! Two different questions with two different costs. `/health` answers the
//! first from nothing at all. The second crosses the Skarbiec broker and
//! provider discovery, so [`check`] runs it on a timer and `/readyz` returns
//! the last completed answer: doing that work inside the request made each
//! three-second deployment probe cancel before a response, then start the same
//! work again.

mod accounts;
pub(in crate::core::server) mod check;
mod placement;

use std::sync::LazyLock;
use std::time::Duration;

use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use serde_json::{json, Value};

/// Readiness is recomputed every thirty seconds.
const PROBE_INTERVAL: Duration = Duration::from_secs(30);

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

/// Return the last completed credential and routing check.
pub(in crate::core::server) async fn readyz() -> impl IntoResponse {
    let report = match READINESS_REPORT.read() {
        Ok(current) => current.clone(),
        Err(poisoned) => poisoned.into_inner().clone(),
    };
    (report.status, Json(report.body))
}

pub(in crate::core::server) fn spawn_readiness_probe() {
    // Off the readiness path on purpose: this shells out to Stado.
    placement::learn();
    tokio::spawn(async {
        loop {
            let report = check::calculate_readiness(publish).await;
            publish(report);
            tokio::time::sleep(PROBE_INTERVAL).await;
        }
    });
}
