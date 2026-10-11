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
    acquire_account, declaration, AcquireOptions, Trigger,
};
use crate::subscription_dispatch::leases;

use super::subscription_pool_scope;

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
/// the operator's limit is refused `409 pool_full` with each member's count,
/// and an acquisition is started for that shortage: its verdict is in the
/// refusal's `details.acquisition`, so the caller reads what was bought or
/// why not.
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
    match leases::take(&request, scope.agent()).await {
        Ok(taken) => Ok(Json(serde_json::to_value(taken).map_err(|error| {
            api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("the lease cannot be encoded: {error}"),
            )
        })?)),
        Err(refused @ leases::Refused::PoolFull { .. }) => {
            let acquisition = match acquire_account(AcquireOptions {
                provider: request.provider.clone(),
                reason: refused.detail(),
                trigger: Trigger::SessionsFull,
                progress: None,
            })
            .await
            {
                Ok(verdict) => verdict,
                Err(error) => json!({"result": "undecidable", "detail": error}),
            };
            Err(api_error_with_details(
                StatusCode::CONFLICT,
                "pool_full",
                &refused.detail(),
                json!({"refusal": refused, "acquisition": acquisition}),
            ))
        }
        Err(refused @ leases::Refused::Incomplete { .. }) => Err(api_error_with_details(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            &refused.detail(),
            json!({"refusal": refused}),
        )),
        Err(refused @ leases::Refused::LimitUnstated { .. }) => Err(api_error_with_details(
            StatusCode::SERVICE_UNAVAILABLE,
            "limit_unstated",
            &refused.detail(),
            json!({"refusal": refused}),
        )),
        Err(refused @ leases::Refused::NoUsableMember { .. }) => Err(api_error_with_details(
            StatusCode::SERVICE_UNAVAILABLE,
            "no_usable_member",
            &refused.detail(),
            json!({"refusal": refused}),
        )),
        Err(refused @ leases::Refused::Unavailable { .. }) => Err(api_error_with_details(
            StatusCode::SERVICE_UNAVAILABLE,
            "dependency_unavailable",
            &refused.detail(),
            json!({"refusal": refused}),
        )),
    }
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
