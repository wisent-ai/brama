//! Adopting somebody else's configuration file: the review the console shows
//! first, and the merge it performs once an operator has picked which aliases
//! to take. Both refuse outright when no runtime registry is configured, since
//! there would be nowhere to adopt into.

use axum::extract::Extension;
use axum::http::StatusCode;
use axum::Json;

use crate::core::server::admission::identity::ModelClientIdentity;
use crate::core::server::aliases::table::ModelAliases;
use crate::core::server::refusal::{api_error, ApiError};

use super::{require_brama_desktop, AdminAdoptionApply, AdminAdoptionPreview};

pub(in crate::core::server) async fn preview_admin_adoption(
    Extension(client_identity): Extension<ModelClientIdentity>,
    Extension(aliases): Extension<ModelAliases>,
    Json(request): Json<AdminAdoptionPreview>,
) -> Result<Json<crate::config_adoption::AdoptionPreview>, ApiError> {
    require_brama_desktop(&client_identity)?;
    let path = aliases.routes_file.as_deref().ok_or_else(|| {
        api_error(
            StatusCode::CONFLICT,
            "runtime route registry is not configured",
        )
    })?;
    crate::config_adoption::preview_document(
        &request.document,
        &request.source_name,
        path,
        &request.agent_id,
    )
    .await
    .map(Json)
    .map_err(|error| api_error(StatusCode::BAD_REQUEST, &error))
}

pub(in crate::core::server) async fn apply_admin_adoption(
    Extension(client_identity): Extension<ModelClientIdentity>,
    Extension(aliases): Extension<ModelAliases>,
    Json(request): Json<AdminAdoptionApply>,
) -> Result<Json<crate::config_adoption::AdoptionResult>, ApiError> {
    require_brama_desktop(&client_identity)?;
    let path = aliases.routes_file.as_deref().ok_or_else(|| {
        api_error(
            StatusCode::CONFLICT,
            "runtime route registry is not configured",
        )
    })?;
    crate::config_adoption::apply_document(
        &request.document,
        &request.source_name,
        path,
        &request.agent_id,
        &request.selected_aliases,
        request.replace_alias_conflicts,
    )
    .await
    .map(Json)
    .map_err(|error| api_error(StatusCode::BAD_REQUEST, &error))
}
