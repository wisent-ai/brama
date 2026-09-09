//! `GET /v1/admin/snapshot`: everything the console needs to render this
//! gateway's routing state in one answer — the route registry as written, the
//! provider roster with whether a capability is configured here, and which
//! product owns which boundary.

use axum::extract::Extension;
use axum::http::StatusCode;
use axum::Json;
use serde_json::{json, Value};

use crate::core::server::admission::identity::ModelClientIdentity;
use crate::core::server::aliases::table::ModelAliases;
use crate::core::server::refusal::{api_error, ApiError};
use crate::core::server::telemetry::stats::wire_protocol_name;

use super::require_brama_desktop;

pub(in crate::core::server) async fn admin_snapshot(
    Extension(client_identity): Extension<ModelClientIdentity>,
    Extension(aliases): Extension<ModelAliases>,
) -> Result<Json<Value>, ApiError> {
    require_brama_desktop(&client_identity)?;
    let routes = match aliases.routes_file.as_deref() {
        Some(path) => crate::core::inference_routes::snapshot(path).map_err(|_| {
            api_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "route registry unavailable",
            )
        })?,
        None => json!({
            "routes": aliases.routes,
            "deployments": [],
        }),
    };
    let providers = crate::providers::adapter::providers()
        .iter()
        .map(|provider| {
            json!({
                "id": provider.id,
                "displayName": provider.display_name,
                "wireProtocol": wire_protocol_name(provider.wire),
                "configured": crate::gateway::broker::provider_capability_configured(provider.id),
            })
        })
        .collect::<Vec<_>>();
    Ok(Json(json!({
        "schemaVersion": 1,
        "build": crate::build_info::current(),
        "routes": routes,
        "providers": providers,
        "automaticRollback": true,
        "boundaries": {
            "routing": "brama",
            "releases": "stado",
            "credentials": "skarbiec",
        },
    })))
}
