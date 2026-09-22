//! `PUT` and `DELETE /v1/admin/categories`: the operator declaring how the
//! catalogue is grouped, or retiring a grouping.
//!
//! A category is data, not code, which is the whole point of it: adding
//! `uncensored` — or any other name this deployment wants to filter on —
//! is a registry write, not a gateway release. The write takes the same
//! staged, validated, owner-only path every route edit takes, so a document
//! that would not load is refused before it replaces the one that does.

use axum::extract::Extension;
use axum::http::StatusCode;
use axum::Json;
use serde_json::{json, Value};

use crate::core::inference_routes::categories::{delete_category, set_category, Category};
use crate::core::server::admission::identity::ModelClientIdentity;
use crate::core::server::aliases::table::ModelAliases;
use crate::core::server::refusal::{api_error, ApiError};

use super::super::{require_brama_desktop, AdminCategoryDelete, AdminCategoryUpdate};

pub(in crate::core::server) async fn update_admin_category(
    Extension(client_identity): Extension<ModelClientIdentity>,
    Extension(aliases): Extension<ModelAliases>,
    Json(request): Json<AdminCategoryUpdate>,
) -> Result<Json<Value>, ApiError> {
    require_brama_desktop(&client_identity)?;
    let path = aliases.routes_file.as_deref().ok_or_else(|| {
        api_error(
            StatusCode::CONFLICT,
            "runtime route registry is not configured",
        )
    })?;
    let category = Category {
        providers: request.providers,
        routes: request.routes,
        terms: request.terms,
    };
    // The refusal the registry produced is the answer: it names the category,
    // the member and what was wrong with it, which a generic "rejected" would
    // have thrown away in front of the one person who can fix it.
    let document = set_category(path, &request.category, &category)
        .map_err(|error| api_error(StatusCode::BAD_REQUEST, &error))?;
    Ok(Json(json!({"ok": true, "routes": document})))
}

pub(in crate::core::server) async fn delete_admin_category(
    Extension(client_identity): Extension<ModelClientIdentity>,
    Extension(aliases): Extension<ModelAliases>,
    Json(request): Json<AdminCategoryDelete>,
) -> Result<Json<Value>, ApiError> {
    require_brama_desktop(&client_identity)?;
    let path = aliases.routes_file.as_deref().ok_or_else(|| {
        api_error(
            StatusCode::CONFLICT,
            "runtime route registry is not configured",
        )
    })?;
    let document = delete_category(path, &request.category).map_err(|error| {
        if error.starts_with("no model category") {
            api_error(StatusCode::NOT_FOUND, &error)
        } else {
            api_error(StatusCode::CONFLICT, &error)
        }
    })?;
    Ok(Json(json!({"ok": true, "routes": document})))
}
