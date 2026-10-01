//! The two operator-driven acts against the pool that cost something: spending
//! one minimal completion to learn whether a provider will serve an account,
//! and asking a provider to refresh its pooled grants.

use axum::extract::{Extension, Path};
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::core::server::administration::require_brama_desktop;
use crate::core::server::admission::identity::{valid_agent_id, ModelClientIdentity};
use crate::core::server::refusal::{api_error, ApiError};
use crate::subscription_dispatch::pool;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::core::server) struct RefreshSubscriptionPoolRequest {
    provider: Option<String>,
    reason: Option<String>,
}

/// Spend one minimal completion against one subscription, because an operator
/// asked whether the provider will actually serve it.
///
/// Nothing else in the gateway spends quota to learn a statistic: plan windows
/// arrive from each provider's own free usage report. This is the deliberate
/// exception, and it is a route rather than a subcommand because redeeming the
/// credential needs the capabilities and identities the launcher installed in
/// this serving process -- a standalone desktop install holds its provider
/// credentials only in this process's memory -- and because the console that
/// renders the verdict is where the question is asked.
///
/// A refusal to spend is a `409`, not a `500`: an account inside a recorded
/// rate-limit block already told us it is out of quota, and the block exists to
/// stop us paying to hear it twice.
pub(in crate::core::server) async fn probe_admin_subscription(
    Extension(client_identity): Extension<ModelClientIdentity>,
    Path((agent_id, subscription_id)): Path<(String, String)>,
) -> Result<Json<Value>, ApiError> {
    require_brama_desktop(&client_identity)?;
    if !valid_agent_id(&agent_id) {
        return Err(api_error(StatusCode::BAD_REQUEST, "invalid agent id"));
    }
    let entry = crate::gateway::broker::discover_subscriptions(&agent_id)
        .await
        .map_err(|detail| api_error(StatusCode::SERVICE_UNAVAILABLE, &detail))?
        .into_iter()
        .find(|entry| entry.id == subscription_id)
        .ok_or_else(|| api_error(StatusCode::NOT_FOUND, "subscription not found"))?;
    let probe = crate::subscription_dispatch::probe::probe_once(&entry.id, &entry.provider)
        .await
        .map_err(|message| api_error(StatusCode::CONFLICT, &message))?;
    Ok(Json(json!({
        "ok": true,
        "probe": probe,
        "subscription": pool::subscription_view(&entry),
    })))
}

/// One operator-driven refresh of a provider's pooled grants, run by the
/// serving process so the attempt shares the sweep's code path and its audit
/// record. A verdict whose result is not `refreshed` is still a 200: the body
/// is the report the operator came for, and the failure it names belongs to
/// the provider, not to this endpoint.
pub(in crate::core::server) async fn refresh_admin_subscription_pool(
    Extension(client_identity): Extension<ModelClientIdentity>,
    Json(request): Json<RefreshSubscriptionPoolRequest>,
) -> Result<Json<Value>, ApiError> {
    require_brama_desktop(&client_identity)?;
    crate::subscription_dispatch::pool::refresh_provider(
        &request.provider.unwrap_or_default(),
        &request.reason.unwrap_or_default(),
    )
    .await
    .map(Json)
    .map_err(|message| api_error(StatusCode::BAD_REQUEST, &message))
}

/// One maintenance pass of this serving process, the work it used to run on
/// its own timers: read every plan usage report that has aged out, renew every
/// grant inside its expiry window, and take a fresh readiness reading. The host's
/// Stado schedule decides how often; `brama maintain` is the caller. `ok` is
/// false when any step failed, and each failure is in the body with its error.
pub(in crate::core::server) async fn maintain_admin(
    Extension(client_identity): Extension<ModelClientIdentity>,
) -> Result<Json<Value>, ApiError> {
    require_brama_desktop(&client_identity)?;
    let plan_usage = crate::subscription_dispatch::plan_usage::sweep().await;
    let credentials = crate::subscription_dispatch::refresh_sweep::sweep().await;
    let readiness = crate::core::server::readiness::recompute().await;
    let usage_failed = plan_usage
        .get("failed")
        .and_then(Value::as_array)
        .is_some_and(|failed| !failed.is_empty());
    let ready = readiness.get("ready").and_then(Value::as_bool) == Some(true);
    let ok = !usage_failed && credentials.is_ok() && ready;
    Ok(Json(json!({
        "ok": ok,
        "plan_usage": plan_usage,
        "credentials": match credentials {
            Ok(report) => report,
            Err(error) => json!({"error": error}),
        },
        "readiness": readiness,
    })))
}
