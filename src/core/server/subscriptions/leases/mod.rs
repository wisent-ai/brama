//! Leases: which live session runs on which subscription, taken before a
//! harness starts and released when its terminal ends. The gateway keeps the
//! register because the pool is its own and every machine's sessions share it.

use axum::extract::{Extension, Path};
use axum::http::{HeaderMap, StatusCode};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::core::server::admission::identity::ModelClientIdentity;
use crate::core::server::refusal::{api_error, api_error_with_details, ApiError};
use crate::subscription_dispatch::acquire::{
    acquire_account, declaration, AcquireOptions, Trigger, ACQUIRED,
};
use crate::subscription_dispatch::leases;

use super::subscription_pool_scope;

mod refused;
pub(in crate::core::server) use refused::report_refused;

/// The live leases the caller may see, with the sessions each subscription
/// carries and each declared provider's limit: `limits` holds the stated
/// ones, `unstated` the refusal of each provider whose declaration states
/// none. The console sees every lease; an agent sees only the leases it took.
/// The counts are the pool's, as the pool document shows them to every caller.
pub(in crate::core::server) async fn list_leases(
    Extension(client_identity): Extension<ModelClientIdentity>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let scope = subscription_pool_scope(&client_identity, &headers, &[]).await?;
    let live: Vec<leases::Lease> = leases::live()
        .map_err(|detail| api_error(StatusCode::SERVICE_UNAVAILABLE, &detail))?
        .into_iter()
        .filter(|lease| lease.belongs_to(scope.agent()))
        .collect();
    let counts =
        leases::counts().map_err(|detail| api_error(StatusCode::SERVICE_UNAVAILABLE, &detail))?;
    let mut limits = serde_json::Map::new();
    let mut unstated = serde_json::Map::new();
    for provider in declaration::declared_providers() {
        match declaration::sessions_cap(provider) {
            Ok(limit) => limits.insert(provider.to_string(), json!(limit)),
            Err(detail) => unstated.insert(provider.to_string(), json!(detail)),
        };
    }
    Ok(Json(json!({
        "scope": scope.named(),
        "limits": limits,
        "unstated": unstated,
        "leases": live,
        "counts": counts,
    })))
}

/// Take a lease for one session. A pool whose every usable member carries
/// the operator's limit (`409 pool_full`), or whose members none can serve
/// (`503 no_usable_member`), starts an acquisition for that shortage. When
/// the acquisition bought an account, the lease is taken again and the
/// session runs on the bought account, with the acquisition's verdict beside
/// the lease; otherwise the refusal carries the verdict in
/// `details.acquisition`, so the caller reads what was bought or why not.
pub(in crate::core::server) async fn take_lease(
    Extension(client_identity): Extension<ModelClientIdentity>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<Json<Value>, ApiError> {
    let scope = subscription_pool_scope(&client_identity, &headers, &body).await?;
    let request: leases::Request = serde_json::from_slice(&body).map_err(|error| {
        api_error(
            StatusCode::BAD_REQUEST,
            &format!(
                "invalid lease request: {error}; a lease names provider, session_id and holder"
            ),
        )
    })?;
    let refused = match leases::take(&request, scope.agent()).await {
        Ok(taken) => return taken_json(&taken, None),
        Err(refused) => refused,
    };
    let (status, code, trigger) = match &refused {
        leases::Refused::PoolFull { .. } => {
            (StatusCode::CONFLICT, "pool_full", Trigger::SessionsFull)
        }
        leases::Refused::NoUsableMember { .. } => (
            StatusCode::SERVICE_UNAVAILABLE,
            "no_usable_member",
            Trigger::NoUsableMember,
        ),
        leases::Refused::Incomplete { .. } => {
            return Err(refusal(
                StatusCode::BAD_REQUEST,
                "invalid_request",
                &refused,
                None,
            ))
        }
        leases::Refused::LimitUnstated { .. } => {
            return Err(refusal(
                StatusCode::SERVICE_UNAVAILABLE,
                "limit_unstated",
                &refused,
                None,
            ))
        }
        leases::Refused::Unavailable { .. } => {
            return Err(refusal(
                StatusCode::SERVICE_UNAVAILABLE,
                "dependency_unavailable",
                &refused,
                None,
            ))
        }
    };
    let acquisition = match acquire_account(AcquireOptions {
        provider: request.provider.clone(),
        reason: refused.detail(),
        trigger,
        progress: None,
    })
    .await
    {
        Ok(verdict) => verdict,
        Err(error) => json!({"result": "undecidable", "detail": error}),
    };
    if acquisition["result"] != json!(ACQUIRED) {
        return Err(refusal(status, code, &refused, Some(acquisition)));
    }
    // The bought account is in the pool now; the same session takes its
    // lease again, and the account that ends it is the one just bought or
    // whichever member the bought one freed.
    match leases::take(&request, scope.agent()).await {
        Ok(taken) => taken_json(&taken, Some(acquisition)),
        Err(after) => Err(api_error_with_details(
            status,
            code,
            &format!(
                "an account was bought for this shortage, and the lease was refused again: {}",
                after.detail()
            ),
            json!({"refusal": after, "first_refusal": refused, "acquisition": acquisition}),
        )),
    }
}

/// A lease taken, with the acquisition that made room for it when one did.
fn taken_json(taken: &leases::Taken, acquisition: Option<Value>) -> Result<Json<Value>, ApiError> {
    let mut answer = serde_json::to_value(taken).map_err(|error| {
        api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("the lease cannot be encoded: {error}"),
        )
    })?;
    if let Some(acquisition) = acquisition {
        answer["acquisition"] = acquisition;
    }
    Ok(Json(answer))
}

fn refusal(
    status: StatusCode,
    code: &str,
    refused: &leases::Refused,
    acquisition: Option<Value>,
) -> ApiError {
    let mut details = json!({"refusal": refused});
    if let Some(acquisition) = acquisition {
        details["acquisition"] = acquisition;
    }
    api_error_with_details(status, code, &refused.detail(), details)
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::core::server) struct ReleaseRequest {
    #[serde(default)]
    lease_id: Option<String>,
    #[serde(default)]
    session_id: Option<String>,
}

/// Release one lease by id, or every live lease of one session. An agent ends
/// only the leases it took: naming another identity's lease is refused `403`
/// and nothing is released.
pub(in crate::core::server) async fn release_leases(
    Extension(client_identity): Extension<ModelClientIdentity>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<Json<Value>, ApiError> {
    let scope = subscription_pool_scope(&client_identity, &headers, &body).await?;
    let request: ReleaseRequest = serde_json::from_slice(&body).map_err(|error| {
        api_error(
            StatusCode::BAD_REQUEST,
            &format!("invalid release request: {error}; a release names lease_id or session_id"),
        )
    })?;
    let released = leases::release(
        request.lease_id.as_deref(),
        request.session_id.as_deref(),
        scope.agent(),
    )
    .map_err(release_refusal)?;
    Ok(Json(json!({ "released": released })))
}

/// Release one lease named in the path, under the same rule.
pub(in crate::core::server) async fn release_lease(
    Extension(client_identity): Extension<ModelClientIdentity>,
    headers: HeaderMap,
    Path(lease_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let scope = subscription_pool_scope(&client_identity, &headers, &[]).await?;
    let released =
        leases::release(Some(&lease_id), None, scope.agent()).map_err(release_refusal)?;
    Ok(Json(json!({ "released": released })))
}

fn release_refusal(refused: leases::ReleaseRefused) -> ApiError {
    match refused {
        leases::ReleaseRefused::Invalid(detail) => api_error(StatusCode::BAD_REQUEST, &detail),
        leases::ReleaseRefused::Unavailable(detail) => {
            api_error(StatusCode::SERVICE_UNAVAILABLE, &detail)
        }
        leases::ReleaseRefused::Foreign(detail) => api_error(StatusCode::FORBIDDEN, &detail),
    }
}
