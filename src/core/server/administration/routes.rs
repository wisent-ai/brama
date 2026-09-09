//! `PUT` and `DELETE /v1/admin/routes`: the operator declaring where one alias
//! points, or removing it.
//!
//! An alias carries exactly one route, so an update names exactly one and is
//! refused whole if that route is unsupported here. A required alias cannot be
//! deleted at all: the gateway would refuse to start without it.

use axum::extract::Extension;
use axum::http::StatusCode;
use axum::Json;
use serde_json::{json, Value};

use crate::core::server::admission::identity::ModelClientIdentity;
use crate::core::server::aliases::table::ModelAliases;
use crate::core::server::aliases::MODEL_ALIASES;
use crate::core::server::refusal::{api_error, ApiError};

use super::{
    require_brama_desktop, route_supported, valid_alias, AdminRouteDelete, AdminRouteUpdate,
};

pub(in crate::core::server) async fn update_admin_route(
    Extension(client_identity): Extension<ModelClientIdentity>,
    Extension(aliases): Extension<ModelAliases>,
    Json(request): Json<AdminRouteUpdate>,
) -> Result<Json<Value>, ApiError> {
    require_brama_desktop(&client_identity)?;
    if !valid_alias(&request.alias) {
        return Err(api_error(StatusCode::BAD_REQUEST, "invalid route alias"));
    }
    if !route_supported(&request.alias, &request.primary) {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "route is unsupported or unavailable",
        ));
    }
    let path = aliases.routes_file.as_deref().ok_or_else(|| {
        api_error(
            StatusCode::CONFLICT,
            "runtime route registry is not configured",
        )
    })?;
    let routes =
        crate::core::inference_routes::update_route(path, &request.alias, &request.primary)
            .map_err(|_| api_error(StatusCode::CONFLICT, "route update was rejected"))?;
    Ok(Json(json!({"ok": true, "routes": routes})))
}

pub(in crate::core::server) async fn delete_admin_route(
    Extension(client_identity): Extension<ModelClientIdentity>,
    Extension(aliases): Extension<ModelAliases>,
    Json(request): Json<AdminRouteDelete>,
) -> Result<Json<Value>, ApiError> {
    require_brama_desktop(&client_identity)?;
    if !valid_alias(&request.alias) {
        return Err(api_error(StatusCode::BAD_REQUEST, "invalid route alias"));
    }
    if MODEL_ALIASES.contains(&request.alias.as_str()) {
        return Err(api_error(
            StatusCode::CONFLICT,
            "required route aliases cannot be deleted",
        ));
    }
    let path = aliases.routes_file.as_deref().ok_or_else(|| {
        api_error(
            StatusCode::CONFLICT,
            "runtime route registry is not configured",
        )
    })?;
    let routes =
        crate::core::inference_routes::delete_route(path, &request.alias).map_err(|error| {
            if error == "route alias not found" {
                api_error(StatusCode::NOT_FOUND, &error)
            } else {
                api_error(StatusCode::CONFLICT, "route deletion was rejected")
            }
        })?;
    Ok(Json(json!({"ok": true, "routes": routes})))
}
