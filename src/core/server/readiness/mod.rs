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
}

static READINESS_REPORT: LazyLock<tokio::sync::RwLock<ReadinessReport>> =
    LazyLock::new(|| tokio::sync::RwLock::new(ReadinessReport::pending()));

/// Return the last completed credential and routing check.
pub(in crate::core::server) async fn readyz() -> impl IntoResponse {
    let report = READINESS_REPORT.read().await.clone();
    (report.status, Json(report.body))
}

pub(in crate::core::server) fn spawn_readiness_probe() {
    tokio::spawn(async {
        loop {
            let report = check::calculate_readiness().await;
            *READINESS_REPORT.write().await = report;
            tokio::time::sleep(Duration::from_secs(30)).await;
        }
    });
}
