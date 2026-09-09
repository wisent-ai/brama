//! `/v1/admin/credentials`: the standalone credential store a desktop install
//! keeps for itself, listed, written and removed. Every one of these refuses
//! with `409` where that store is not enabled, because on a launcher-provisioned
//! host the credentials belong to Skarbiec and not to this process.

use axum::extract::Extension;
use axum::http::StatusCode;
use axum::Json;
use serde_json::{json, Value};

use crate::core::server::admission::identity::ModelClientIdentity;
use crate::core::server::refusal::{api_error, ApiError};
use crate::core::server::subscriptions::account::account_credential_provider;

use super::{require_brama_desktop, AdminCredentialMutation};

pub(in crate::core::server) async fn list_admin_credentials(
    Extension(client_identity): Extension<ModelClientIdentity>,
) -> Result<Json<Value>, ApiError> {
    require_brama_desktop(&client_identity)?;
    let providers = crate::gateway::broker::local_provider_names().map_err(|_| {
        api_error(
            StatusCode::CONFLICT,
            "standalone credential store is not enabled",
        )
    })?;
    Ok(Json(json!({"providers": providers})))
}

pub(in crate::core::server) async fn put_admin_credential(
    Extension(client_identity): Extension<ModelClientIdentity>,
    Json(request): Json<AdminCredentialMutation>,
) -> Result<Json<Value>, ApiError> {
    require_brama_desktop(&client_identity)?;
    let provider = account_credential_provider(Some(&request.provider))
        .await
        .ok_or_else(|| {
            api_error(
                StatusCode::BAD_REQUEST,
                "provider must name a supported remote API provider",
            )
        })?;
    let credential = request.credential.as_deref().unwrap_or("");
    crate::gateway::broker::put_local_provider_credential(&provider, credential)
        .map_err(|error| api_error(StatusCode::BAD_REQUEST, &error))?;
    Ok(Json(json!({"ok": true, "provider": provider})))
}

pub(in crate::core::server) async fn delete_admin_credential(
    Extension(client_identity): Extension<ModelClientIdentity>,
    Json(request): Json<AdminCredentialMutation>,
) -> Result<Json<Value>, ApiError> {
    require_brama_desktop(&client_identity)?;
    let removed = crate::gateway::broker::remove_local_provider_credential(&request.provider)
        .map_err(|_| {
            api_error(
                StatusCode::CONFLICT,
                "standalone credential store is not enabled",
            )
        })?;
    if !removed {
        return Err(api_error(
            StatusCode::NOT_FOUND,
            "provider credential not found",
        ));
    }
    Ok(Json(json!({"ok": true, "provider": request.provider})))
}
