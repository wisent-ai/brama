//! The operator-driven acts against the pool that need the serving process:
//! spending one minimal completion to learn whether a provider will serve an
//! account, asking a provider to refresh its pooled grants, and recording
//! which account each member belongs to in the vault this process serves from.

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

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::core::server) struct AttributeSubscriptionPoolRequest {
    provider: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::core::server) struct SecondFactorRequest {
    provider: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::core::server) struct AcquireSubscriptionRequest {
    provider: String,
    reason: String,
}

/// Buy one account of a provider when every pool account has spent its plan
/// and the pool holds fewer accounts than the operator allows, run by the
/// serving process because it holds the vault, the ledger and the journal.
/// A refusal or a failed purchase is still a 200: the verdict is the report
/// the operator came for, and `result` says how it ended. A request that
/// cannot be decided (an unknown provider, no reason) is a 400.
pub(in crate::core::server) async fn acquire_admin_subscription(
    Extension(client_identity): Extension<ModelClientIdentity>,
    Json(request): Json<AcquireSubscriptionRequest>,
) -> Result<Json<Value>, ApiError> {
    require_brama_desktop(&client_identity)?;
    crate::subscription_dispatch::acquire::acquire_account(
        crate::subscription_dispatch::acquire::AcquireOptions {
            provider: request.provider,
            reason: request.reason,
            trigger: crate::subscription_dispatch::acquire::Trigger::Operator,
            progress: None,
        },
    )
    .await
    .map(Json)
    .map_err(|message| api_error(StatusCode::BAD_REQUEST, &message))
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
    // Both are the operator's to state: a refresh names the provider whose
    // grants it renews and why, and the audit record keeps both.
    let Some(provider) = request.provider else {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "a pool refresh names the provider whose grants it renews: provider is missing",
        ));
    };
    let Some(reason) = request.reason else {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "a pool refresh says why it runs: reason is missing",
        ));
    };
    crate::subscription_dispatch::pool::refresh_provider(&provider, &reason)
        .await
        .map(Json)
        .map_err(|message| api_error(StatusCode::BAD_REQUEST, &message))
}

/// Record which account each member of one provider belongs to, in the vault
/// this serving process reads, which is the vault Weles resolves a sign-in
/// from. The same attribution run from a workstation shell writes whatever
/// vault that shell's program opens, so the account never reached a sign-in.
/// Members left unattributed are in the 200 body with their reasons; a
/// provider with no member is a 400.
pub(in crate::core::server) async fn attribute_admin_subscription_pool(
    Extension(client_identity): Extension<ModelClientIdentity>,
    Json(request): Json<AttributeSubscriptionPoolRequest>,
) -> Result<Json<Value>, ApiError> {
    require_brama_desktop(&client_identity)?;
    pool::record_accounts(&request.provider)
        .await
        .map(Json)
        .map_err(|message| api_error(StatusCode::BAD_REQUEST, &message))
}

/// Which accounts need a second factor and which hold the authenticator seed
/// that answers it, read from the vault this serving process reads: the
/// vault Weles signs in from. A machine that reads the fleet vault remotely
/// holds no vault its own `skarbiec totp-seed-state` can open, so the report
/// is asked of the gateway there. A vault that cannot be read is a 503 naming
/// the vault program's own error.
pub(in crate::core::server) async fn second_factor_admin_subscription_pool(
    Extension(client_identity): Extension<ModelClientIdentity>,
    Json(request): Json<SecondFactorRequest>,
) -> Result<Json<Value>, ApiError> {
    require_brama_desktop(&client_identity)?;
    pool::second_factor_report(request.provider.as_deref())
        .await
        .map(Json)
        .map_err(|message| api_error(StatusCode::SERVICE_UNAVAILABLE, &message))
}

/// One maintenance pass of this serving process ([`super::maintenance::pass`]),
/// asked by `brama maintain` on the host's Stado schedule.
pub(in crate::core::server) async fn maintain_admin(
    Extension(client_identity): Extension<ModelClientIdentity>,
    supplied: Option<Json<crate::subscription_dispatch::discovery::harness::DiscoveryReport>>,
) -> Result<Json<Value>, ApiError> {
    require_brama_desktop(&client_identity)?;
    Ok(Json(
        super::maintenance::pass(supplied.map(|Json(report)| report)).await,
    ))
}

/// Register independently observed account metadata; sign-in stays with Weles.
pub(in crate::core::server) async fn discover_admin_subscription_pool(
    Extension(client_identity): Extension<ModelClientIdentity>,
    Json(report): Json<crate::subscription_dispatch::discovery::harness::DiscoveryReport>,
) -> Result<Json<Value>, ApiError> {
    require_brama_desktop(&client_identity)?;
    Ok(Json(
        crate::subscription_dispatch::discovery::discover(report).await,
    ))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::core::server) struct ResetSubscriptionRequest {
    provider: String,
    member: String,
    reason: String,
}

pub(in crate::core::server) async fn reset_admin_subscription(
    Extension(client_identity): Extension<ModelClientIdentity>,
    Json(request): Json<ResetSubscriptionRequest>,
) -> Result<Json<Value>, ApiError> {
    require_brama_desktop(&client_identity)?;
    crate::subscription_dispatch::acquire::resets::redeem::run(
        &request.provider,
        &request.member,
        &request.reason,
        false,
    )
    .await
    .map(Json)
    .map_err(|error| api_error(StatusCode::CONFLICT, &error))
}

/// One change to, or reading of, the operator's saved-reset policy.
#[derive(Debug, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub(in crate::core::server) enum ResetPolicyRequest {
    Show,
    Set {
        provider: String,
        keep_credits: u64,
        min_blocked_minutes: u64,
        salvage_horizon_hours: u64,
        reason: String,
    },
    Clear {
        provider: String,
    },
}

/// The operator's saved-reset policy on the gateway that holds the pool, as
/// `brama subscription reset-policy` and Brama Desktop state and read it.
pub(in crate::core::server) async fn reset_policy_admin(
    Extension(client_identity): Extension<ModelClientIdentity>,
    Json(request): Json<ResetPolicyRequest>,
) -> Result<Json<Value>, ApiError> {
    use crate::subscription_dispatch::acquire::resets::settings;
    require_brama_desktop(&client_identity)?;
    let answer = match request {
        ResetPolicyRequest::Show => settings::all().map(|policies| json!({ "policies": policies })),
        ResetPolicyRequest::Set {
            provider,
            keep_credits,
            min_blocked_minutes,
            salvage_horizon_hours,
            reason,
        } => {
            let policy = settings::ResetPolicy {
                keep_credits,
                min_blocked_minutes,
                salvage_horizon_hours,
                reason,
                stated_at: chrono::Utc::now().to_rfc3339(),
            };
            settings::set(&provider, policy)
                .map(|policy| json!({ "provider": provider, "policy": policy }))
        }
        ResetPolicyRequest::Clear { provider } => settings::clear(&provider)
            .map(|policy| json!({ "provider": provider, "cleared": policy })),
    };
    answer
        .map(Json)
        .map_err(|error| api_error(StatusCode::CONFLICT, &error))
}
