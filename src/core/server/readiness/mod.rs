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
pub(in crate::core::server) mod standing;

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
                "inventory_errors": [],
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
        provider_inventory_error: Option<&str>,
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
                "inventory_errors": provider_inventory_error.map(|detail| json!({
                    "operation": "configured_provider_capabilities", "detail": detail,
                })).into_iter().collect::<Vec<_>>(),
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
    *crate::core::sync::write_held(&READINESS_REPORT, "readiness report") = report;
}

/// The last `brama maintain` pass this process ran: when it finished, and
/// whether every step succeeded. `None` until the first pass.
static LAST_MAINTENANCE: LazyLock<std::sync::RwLock<Option<(std::time::SystemTime, bool)>>> =
    LazyLock::new(|| std::sync::RwLock::new(None));

/// Record one finished maintenance pass for `/readyz`.
pub(in crate::core::server) fn record_maintenance(ok: bool) {
    let mut last = crate::core::sync::write_held(&LAST_MAINTENANCE, "last maintenance pass");
    *last = Some((std::time::SystemTime::now(), ok));
}

/// What `/readyz` says about maintenance. The gateway keeps no timer, so it
/// cannot tell a late pass from an early one; it states when the last pass
/// ended and how, and says plainly when none has run. A pass runs when the
/// host's Stado schedule calls `brama maintain`, and when a request is
/// refused for want of a signed-in subscription.
fn maintenance_state() -> Value {
    let last = *crate::core::sync::read_held(&LAST_MAINTENANCE, "last maintenance pass");
    match last {
        Some((finished, ok)) => match finished.duration_since(std::time::UNIX_EPOCH) {
            Ok(elapsed) => json!({"last_pass_unix": elapsed.as_secs(), "last_pass_ok": ok}),
            Err(error) => json!({
                "last_pass_unix": null,
                "last_pass_ok": ok,
                "detail": format!("this host's clock put the last pass before the Unix epoch: {error}"),
            }),
        },
        None => json!({
            "last_pass_unix": null,
            "detail": "no maintenance pass has run since this process started: one runs when the \
                       host's Stado schedule calls brama maintain, and when a request is refused \
                       for want of a signed-in subscription; a host with no such schedule \
                       declares it with stado repair brama --step declarations --target <host> --apply"
        }),
    }
}

/// Return the last completed credential and routing check, with the last
/// maintenance pass beside it. A credential call standing unanswered past
/// the declared refresh interval overrides it: the gateway declares it
/// serves nothing and names the source (`standing`).
pub(in crate::core::server) async fn readyz() -> impl IntoResponse {
    let report = {
        let current = crate::core::sync::read_held(&READINESS_REPORT, "readiness report");
        current.clone()
    };
    let mut status = report.status;
    let mut body = report.body;
    if let Some(object) = body.as_object_mut() {
        object.insert("maintenance".to_string(), maintenance_state());
        // Calls that stand and cannot be measured, because the resolver's
        // declaration is unreadable, refuse the same way with that cause.
        let refusal = match standing::refusal() {
            Ok(refusal) => refusal,
            Err(cause) => Some(cause),
        };
        if let Some(refusal) = refusal {
            status = StatusCode::SERVICE_UNAVAILABLE;
            object.insert("ready".to_string(), Value::Bool(false));
            object.insert("reason".to_string(), Value::String(refusal.clone()));
            object.insert("credential_source".to_string(), Value::String(refusal));
        }
    }
    (status, Json(body))
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
