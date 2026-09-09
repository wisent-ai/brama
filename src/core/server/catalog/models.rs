//! `GET /v1/models`: every id this caller may name, and whether this gateway
//! can actually serve it.
//!
//! Four sources are merged: the public vendor catalogue, the models the
//! caller's own subscriptions discover, the routes its bearer is restricted to,
//! and every declared alias. An alias that cannot serve is listed with the
//! reason rather than dropped, because a name that silently vanishes reads to
//! the caller as "no such model" and sends it to fix the wrong thing.

use std::collections::{HashMap, HashSet};

use axum::extract::Extension;
use axum::Json;
use tracing::warn;

use crate::core::server::admission::identity::{ModelClientIdentity, BRAMA_DESKTOP_CLIENT_ID};
use crate::core::server::admission::{authorize_caller, has_caller_auth_headers};
use crate::core::server::aliases::table::ModelAliases;
use crate::core::server::refusal::ApiError;
use crate::core::server::subscriptions::account::account_agent_id;
use crate::subscription_dispatch::registry_models_for_agent;

use super::views::{self, CatalogView};

pub(in crate::core::server) async fn list_models(
    Extension(client_identity): Extension<ModelClientIdentity>,
    Extension(aliases): Extension<ModelAliases>,
    headers: axum::http::HeaderMap,
) -> Result<Json<serde_json::Value>, ApiError> {
    let account_agent = account_agent_id(&client_identity).ok();
    let signed_catalog_agent = if account_agent.is_none() && has_caller_auth_headers(&headers) {
        Some(authorize_caller(&client_identity, &headers, &[], None).await?)
    } else {
        None
    };
    let account_catalog = account_agent.is_some();
    let catalog_agent = account_agent.clone().or(signed_catalog_agent);
    // The same condition decides two disclosures: performance history and
    // whether a model can actually be served. Both are answers about the
    // caller, so neither is given to an unknown caller.
    let caller_known = catalog_agent.is_some();
    // Deployment-owned capabilities never make a route available to a user
    // account. Account availability comes only from that account's stored key.
    let configured_providers = if !account_catalog && catalog_agent.is_some() {
        crate::gateway::broker::configured_provider_capabilities()
    } else {
        HashSet::new()
    };
    let mut model_ids = Vec::new();
    let mut available = HashSet::new();

    let mut registry_metadata = HashMap::new();
    let mut catalog_revision =
        std::env::var("BRAMA_CATALOG_REVISION").unwrap_or_else(|_| "brama-v1".into());
    let mut degraded = false;
    match crate::subscription_dispatch::model_catalog::snapshot().await {
        Ok(catalog) => {
            catalog_revision = catalog.revision.clone();
            for model in &catalog.models {
                model_ids.push(model.route_id.clone());
                if catalog_agent.is_some() && configured_providers.contains(&model.provider_id) {
                    available.insert(model.route_id.clone());
                }
                registry_metadata.insert(model.route_id.clone(), model.clone());
            }
        }
        Err(error) => {
            degraded = true;
            warn!(%error, "public model catalog unavailable");
        }
    }
    if let Some(catalog_agent) = catalog_agent.as_deref() {
        match registry_models_for_agent(catalog_agent).await {
            Ok(models) => {
                for model in models {
                    available.insert(model.route_id.clone());
                    model_ids.push(model.route_id.clone());
                    registry_metadata.insert(model.route_id.clone(), model);
                }
            }
            Err(error) => {
                degraded = true;
                warn!(%error, "native provider model discovery failed");
            }
        }
    }
    // The desktop console holds a bearer, not an agent signature, so the branch
    // above never runs for it and its catalogue arrived carrying the public
    // vendor list alone. That list knows `openai`; it does not know that a
    // `codex` subscription is what pays for those models here, so no screen in
    // the console could name what a subscription covers.
    if client_identity.client_id == BRAMA_DESKTOP_CLIENT_ID {
        match crate::subscription_dispatch::dispatch::registry_models_for_console().await {
            Ok(models) => {
                for model in models {
                    available.insert(model.route_id.clone());
                    model_ids.push(model.route_id.clone());
                    registry_metadata.insert(model.route_id.clone(), model);
                }
            }
            Err(error) => {
                degraded = true;
                warn!(%error, "console provider model discovery failed");
            }
        }
    }
    // A bearer may be intentionally restricted to a small set of canonical
    // direct routes. Those routes remain real and dispatchable even when the
    // optional public catalogue is unavailable, so the authenticated model list
    // must not become empty while inference still works.
    if let Some(allowed_models) = client_identity.allowed_models.as_ref() {
        for route in allowed_models {
            if crate::providers::adapter::provider_id_from_route(route)
                .is_some_and(crate::gateway::broker::provider_capability_configured)
            {
                model_ids.push(route.clone());
                available.insert(route.clone());
            }
        }
    }

    // Every alias the caller may name is listed. One that serves is available;
    // one that is declared and cannot serve is listed as unavailable with the
    // reason, because a name that silently vanishes from the catalogue reads
    // to the caller as "no such model" and sends it to fix the wrong thing.
    let mut unavailable_reasons: HashMap<String, String> = HashMap::new();
    for alias in aliases.declared() {
        if !client_identity.authorizes_model(&alias) {
            continue;
        }
        let diagnosis = aliases.diagnose(&alias);
        model_ids.push(alias.clone());
        if diagnosis.serving() {
            available.insert(alias);
        } else if let Some(reason) = diagnosis.reason {
            unavailable_reasons.insert(alias, reason);
        }
    }
    model_ids.sort();
    model_ids.dedup();
    if account_catalog {
        model_ids
            .retain(|model| crate::providers::adapter::provider_id_from_route(model).is_some());
    } else {
        model_ids.retain(|model| client_identity.authorizes_model(model));
    }

    let view = CatalogView {
        model_ids,
        available,
        registry_metadata,
        unavailable_reasons,
        caller_known,
    };
    if headers.contains_key("x-jeden-schema-min") {
        return Ok(Json(views::jeden(view, catalog_revision, degraded)));
    }
    Ok(Json(views::openai(view)))
}
