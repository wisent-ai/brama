//! What a caller may ask this gateway for.
//!
//! `GET /v1/models` is the caller's view — every id it may name, in the shape
//! its SDK expects, assembled in [`models`] and rendered in [`views`]. The
//! alias list below is the operator's view of the same table: every alias, the
//! route it declares, the state the gateway would put it in right now, and
//! whether the presenting bearer is allowed to use it. It exists because the
//! only place an unroutable alias used to show up was a warning in the server
//! log and a `not in the catalog` sentence in some other product.

pub(in crate::core::server) mod models;
mod views;

use axum::extract::Extension;
use axum::Json;
use serde_json::json;

use crate::core::server::admission::identity::ModelClientIdentity;
use crate::core::server::aliases::diagnosis::ALIAS_SERVING;
use crate::core::server::aliases::table::ModelAliases;
use crate::core::server::aliases::MODEL_ALIASES;
use crate::core::server::refusal::ApiError;

/// Every alias this gateway declares, with whether it serves and why not.
pub(in crate::core::server) async fn list_aliases(
    Extension(client_identity): Extension<ModelClientIdentity>,
    Extension(aliases): Extension<ModelAliases>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let report = aliases
        .declared()
        .iter()
        .map(|alias| {
            let diagnosis = aliases.diagnose(alias);
            json!({
                "alias": diagnosis.alias,
                "state": diagnosis.state,
                "route": diagnosis.route,
                "reason": diagnosis.reason,
                "required": MODEL_ALIASES.contains(&alias.as_str()),
                "authorized": client_identity.authorizes_model(alias),
            })
        })
        .collect::<Vec<_>>();
    let unserviceable = report
        .iter()
        .filter(|entry| entry["state"] != ALIAS_SERVING)
        .count();
    Ok(Json(json!({
        "object": "list",
        "aliases": report,
        "unserviceable": unserviceable,
    })))
}
