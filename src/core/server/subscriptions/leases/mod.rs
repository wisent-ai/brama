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

/// Every live lease, with the sessions each subscription carries and each
/// declared provider's limit: `limits` holds the stated ones, `unstated`
/// the refusal of each provider whose declaration states none.
pub(in crate::core::server) async fn list_leases(
    Extension(client_identity): Extension<ModelClientIdentity>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    subscription_pool_scope(&client_identity, &headers, &[]).await?;
    let live =
        leases::live().map_err(|detail| api_error(StatusCode::SERVICE_UNAVAILABLE, &detail))?;
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
    subscription_pool_scope(&client_identity, &headers, &body).await?;
    let request: leases::Request = serde_json::from_slice(&body).map_err(|error| {
        api_error(
            StatusCode::BAD_REQUEST,
            &format!(
                "invalid lease request: {error}; a lease names provider, session_id and holder"
            ),
        )
    })?;
    match leases::take(&request).await {
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

/// Release one lease by id, or every live lease of one session.
pub(in crate::core::server) async fn release_leases(
    Extension(client_identity): Extension<ModelClientIdentity>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<Json<Value>, ApiError> {
    subscription_pool_scope(&client_identity, &headers, &body).await?;
    let request: ReleaseRequest = serde_json::from_slice(&body).map_err(|error| {
        api_error(
            StatusCode::BAD_REQUEST,
            &format!("invalid release request: {error}; a release names lease_id or session_id"),
        )
    })?;
    let released = leases::release(request.lease_id.as_deref(), request.session_id.as_deref())
        .map_err(|detail| api_error(StatusCode::BAD_REQUEST, &detail))?;
    Ok(Json(json!({ "released": released })))
}

/// Release one lease named in the path.
pub(in crate::core::server) async fn release_lease(
    Extension(client_identity): Extension<ModelClientIdentity>,
    headers: HeaderMap,
    Path(lease_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    subscription_pool_scope(&client_identity, &headers, &[]).await?;
    let released = leases::release(Some(&lease_id), None)
        .map_err(|detail| api_error(StatusCode::BAD_REQUEST, &detail))?;
    Ok(Json(json!({ "released": released })))
}
