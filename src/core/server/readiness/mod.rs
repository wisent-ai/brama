//! Is this process up, and can it carry traffic?
//!
//! Two different questions with two different costs. `/health` answers the
//! first from nothing at all. The second crosses the Skarbiec broker and
//! provider discovery, so [`check`] runs it once when the gateway starts and
//! again on every `brama maintain` pass, and `/readyz` returns the last
//! completed answer.
//!
//! A client waiting on a request Brama is answering asks whether Brama
//! still serves. Asking again the moment each answer returned ran one
//! request per answer for as long as the wait lasted, on the same adapter
//! as the request it watched. `GET /readyz?after=<digest>` answers instead
//! when the readiness answer differs from the one the client names by its
//! digest: at once when it already does, otherwise when something changes
//! it — a sweep or a maintenance pass publishing, a credential call
//! beginning or ending, or a standing call reaching the refresh interval
//! this host's resolver declares, the one change time alone makes. Every
//! answer carries its own `digest`, to be named on the next question.

mod accounts;
pub(in crate::core::server) mod check;
mod placement;
pub(in crate::core::server) mod standing;

use std::sync::LazyLock;

use axum::extract::Query;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::sync::Notify;

/// Told whenever something the readiness answer reads changes.
static CHANGED: LazyLock<Notify> = LazyLock::new(Notify::new);

/// Wake every readiness question waiting for the answer to change.
fn changed() {
    CHANGED.notify_waiters();
}

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
    changed();
}

/// The last maintenance pass this process ran: when it finished, and whether
/// every step succeeded. `None` until the first pass.
static LAST_MAINTENANCE: LazyLock<std::sync::RwLock<Option<(std::time::SystemTime, bool)>>> =
    LazyLock::new(|| std::sync::RwLock::new(None));

/// Record one finished maintenance pass for `/readyz`.
pub(in crate::core::server) fn record_maintenance(ok: bool) {
    {
        let mut last = crate::core::sync::write_held(&LAST_MAINTENANCE, "last maintenance pass");
        *last = Some((std::time::SystemTime::now(), ok));
    }
    changed();
}

/// What `/readyz` says about maintenance: when the last pass ended and how,
/// and plainly when none has finished yet. The serving process runs its own
/// passes (one when it starts, then one at each moment the ledger states
/// something becomes due, and one after a request refused for want of a
/// signed-in subscription); `brama maintain` asks for one more.
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
            "detail": "the serving process's first maintenance pass has not finished since it \
                       started: it renews due grants and signs members in through Weles by itself, \
                       and its result appears here when it ends"
        }),
    }
}

/// What `/readyz` asks for: with `after`, the digest of the answer the
/// client last read, the answer once it differs from that one.
#[derive(serde::Deserialize)]
pub(in crate::core::server) struct ReadinessQuestion {
    after: Option<String>,
}

/// The last completed credential and routing check, with the last
/// maintenance pass beside it and the `digest` that names this answer. A
/// question naming the digest of the current answer is answered when the
/// answer changes.
pub(in crate::core::server) async fn readyz(
    Query(question): Query<ReadinessQuestion>,
) -> impl IntoResponse {
    let (status, body) = answer_after(question.after.as_deref()).await;
    (status, Json(body))
}

/// The digest that names one readiness answer: its status and its body.
fn digest(status: StatusCode, body: &Value) -> String {
    let mut hasher = Sha256::new();
    hasher.update(status.as_str().as_bytes());
    hasher.update(body.to_string().as_bytes());
    format!("{:x}", hasher.finalize())
}

/// The readiness answer, with its `digest`, once it is not the answer
/// `after` names; with no `after`, the answer now.
async fn answer_after(after: Option<&str>) -> (StatusCode, Value) {
    loop {
        // Registered before the answer is read, so a change between the
        // reading and the wait wakes this question instead of passing it.
        let notified = CHANGED.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        let (status, mut body, calls) = answer();
        let named = digest(status, &body);
        if after != Some(named.as_str()) {
            if let Some(object) = body.as_object_mut() {
                object.insert("digest".to_string(), Value::String(named));
                // How long each call has stood changes every second; it is
                // added after the digest, which changes only with the calls.
                let listed = calls.iter().map(|call| call.document(true)).collect();
                object.insert("standing_calls".to_string(), Value::Array(listed));
            }
            return (status, body);
        }
        match standing::next_reading() {
            Some(due) => {
                tokio::select! {
                    () = &mut notified => {}
                    () = tokio::time::sleep_until(tokio::time::Instant::from_std(due)) => {}
                }
            }
            None => notified.await,
        }
    }
}

/// The readiness answer now, and the standing calls it lists. A credential
/// call standing unanswered past the declared refresh interval overrides
/// the last check: the gateway declares it serves nothing and names the
/// source (`standing`). Every call standing is listed in `standing_calls`.
fn answer() -> (StatusCode, Value, Vec<standing::CallReading>) {
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
    let calls = standing::readings();
    if let Some(object) = body.as_object_mut() {
        let listed = calls.iter().map(|call| call.document(false)).collect();
        object.insert("standing_calls".to_string(), Value::Array(listed));
    }
    (status, body, calls)
}

/// Learn this host's placement, take the first readiness reading, once, and
/// start the gateway's own readings of standing credential calls.
pub(in crate::core::server) fn spawn_readiness_probe() {
    // Off the readiness path on purpose: this shells out to Stado.
    placement::learn();
    tokio::spawn(recompute());
    tokio::spawn(standing::watch());
}

/// Take one readiness reading, publish it for `/readyz`, and return its body.
pub(in crate::core::server) async fn recompute() -> Value {
    let report = check::calculate_readiness(publish).await;
    let body = report.body.clone();
    publish(report);
    body
}
