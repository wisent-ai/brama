//! `GET /v1/models`: every id this caller may name, and whether this gateway
//! can actually serve it.
//!
//! Four sources are merged: the public vendor catalogue, the models the
//! caller's own subscriptions discover, the routes its bearer is restricted to,
//! and every declared alias. An alias that cannot serve is listed with the
//! reason rather than dropped, because a name that silently vanishes reads to
//! the caller as "no such model" and sends it to fix the wrong thing.

use std::collections::{HashMap, HashSet};

use axum::extract::{Extension, Query};
use axum::Json;
use tracing::warn;

use crate::core::inference_routes::categories::{categories_for, declared, Categories};
use crate::core::server::admission::identity::{is_console, ModelClientIdentity};
use crate::core::server::admission::{authorize_caller, has_caller_auth_headers};
use crate::core::server::aliases::table::ModelAliases;
use crate::core::server::refusal::{api_error, ApiError};
use crate::core::server::subscriptions::account::account_agent_id;
use crate::providers::adapter::RegistryModel;
use crate::subscription_dispatch::registry_models_for_agent;

use super::filters::CatalogFilters;
use super::views::{self, CatalogView};

pub(in crate::core::server) async fn list_models(
    Extension(client_identity): Extension<ModelClientIdentity>,
    Extension(aliases): Extension<ModelAliases>,
    Query(query): Query<HashMap<String, String>>,
    headers: axum::http::HeaderMap,
) -> Result<Json<serde_json::Value>, ApiError> {
    let filters = CatalogFilters::from_query(query)?.validated()?;
    // Only a request carrying a signed-in Wisent user has an account agent;
    // for one that carries a user, the id is derived, never refused.
    let account_agent = match client_identity.human_context() {
        Some(_) => Some(account_agent_id(&client_identity)?),
        None => None,
    };
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
            .map_err(|error| api_error(axum::http::StatusCode::SERVICE_UNAVAILABLE, &error))?
    } else {
        HashSet::new()
    };
    let mut model_ids = Vec::new();
    let mut available = HashSet::new();

    let mut registry_metadata = HashMap::new();
    let mut catalog_revision = None;
    let mut catalog_source = None;
    let mut degraded = false;
    match crate::subscription_dispatch::model_catalog::snapshot().await {
        Ok(catalog) => {
            catalog_revision = Some(catalog.revision.clone());
            catalog_source = crate::subscription_dispatch::model_catalog::source().await;
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
    // The console holds a bearer, not an agent signature, so the branch above
    // never runs for it and its catalogue arrived carrying the public vendor
    // list alone. That list knows `openai`; it does not know that a `codex`
    // subscription is what pays for those models here, so no screen in the
    // console could name what a subscription covers. A standalone launcher's
    // console (Jeden's own gateway) sends its model work by the same bearer.
    let console = is_console(&client_identity.client_id).map_err(|cause| {
        api_error(
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            &format!("which client is this gateway's console cannot be read: {cause}"),
        )
    })?;
    if console {
        match crate::subscription_dispatch::dispatch::registry_models_for_console().await {
            Ok(models) => {
                for model in models {
                    available.insert(model.route_id.clone());
                    model_ids.push(model.route_id.clone());
                    registry_metadata.insert(model.route_id.clone(), model);
                }
            }
            Err(error) => {
                return Err(api_error(
                    axum::http::StatusCode::SERVICE_UNAVAILABLE,
                    &error,
                ));
            }
        }
    }
    // A bearer may be intentionally restricted to a small set of canonical
    // direct routes. Those routes remain real and dispatchable even when the
    // optional public catalogue is unavailable, so the authenticated model list
    // must not become empty while inference still works.
    let mut unavailable_reasons: HashMap<String, String> = HashMap::new();
    if let Some(allowed_models) = client_identity.allowed_models.as_ref() {
        for route in allowed_models {
            if let Some(provider) = crate::providers::adapter::provider_id_from_route(route) {
                match crate::gateway::broker::provider_capability_configured(provider) {
                    Ok(true) => {
                        model_ids.push(route.clone());
                        available.insert(route.clone());
                    }
                    Ok(false) => {}
                    Err(error) => {
                        degraded = true;
                        model_ids.push(route.clone());
                        available.remove(route);
                        unavailable_reasons.insert(route.clone(), error);
                    }
                }
            }
        }
    }

    // Every alias the caller may name is listed. One that serves is available;
    // one that is declared and cannot serve is listed as unavailable with the
    // reason, because a name that silently vanishes from the catalogue reads
    // to the caller as "no such model" and sends it to fix the wrong thing.
    let declared_aliases = aliases
        .declared()
        .map_err(|error| api_error(axum::http::StatusCode::SERVICE_UNAVAILABLE, &error))?;
    for alias in declared_aliases {
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

    // Categories are the operator's declaration, so they are read from the
    // registry once per request and applied to whatever the caller is allowed
    // to see. An alias carries the categories of the route it points at:
    // asking for the uncensored models and being shown the alias that reaches
    // one is the answer the caller wanted.
    let declared_categories = declared();
    let alias_routes = alias_routes(&aliases, &model_ids, &registry_metadata)?;
    let categories = model_categories(
        &declared_categories,
        &model_ids,
        &registry_metadata,
        &alias_routes,
    );
    if filters.any() {
        model_ids.retain(|id| {
            let carried: &[String] = match categories.get(id) {
                Some(names) => names,
                None => &[],
            };
            filters.matches(
                catalogue_model(id, &registry_metadata, &alias_routes),
                carried,
            )
        });
    }

    let view = CatalogView {
        model_ids,
        available,
        registry_metadata,
        unavailable_reasons,
        categories,
        caller_known,
    };
    if headers.contains_key("x-jeden-schema-min") {
        let mut answer = views::jeden(view, catalog_revision, degraded);
        // Where the public catalogue behind this answer came from: read live,
        // or the copy last written to disk with its time and the live error.
        if let Some(source) = catalog_source {
            answer["catalogSource"] = serde_json::json!(source);
        }
        return Ok(Json(answer));
    }
    Ok(Json(views::openai(view)))
}

/// The catalogue route each listed alias points at, for every listed id the
/// catalogue does not hold itself. An alias that points at delegation rather
/// than a catalogue route has none; a routes file that cannot be read refuses
/// the listing with its reason instead of dropping the alias's categories.
fn alias_routes(
    aliases: &ModelAliases,
    model_ids: &[String],
    registry_metadata: &HashMap<String, RegistryModel>,
) -> Result<HashMap<String, String>, ApiError> {
    let mut routes = HashMap::new();
    for id in model_ids
        .iter()
        .filter(|id| !registry_metadata.contains_key(*id))
    {
        let declared = aliases.declared_route(id).map_err(|error| {
            api_error(
                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                &format!("the route registry could not resolve {id}: {error}"),
            )
        })?;
        if let Some(route) = declared
            .filter(|route| crate::providers::adapter::provider_id_from_route(route).is_some())
        {
            routes.insert(id.clone(), route);
        }
    }
    Ok(routes)
}

/// The catalogue model an id names, directly or through the alias pointing at it.
fn catalogue_model<'a>(
    id: &str,
    registry_metadata: &'a HashMap<String, RegistryModel>,
    alias_routes: &HashMap<String, String>,
) -> Option<&'a RegistryModel> {
    match registry_metadata.get(id) {
        Some(model) => Some(model),
        None => alias_routes
            .get(id)
            .and_then(|route| registry_metadata.get(route)),
    }
}

fn model_categories(
    declared_categories: &Categories,
    model_ids: &[String],
    registry_metadata: &HashMap<String, RegistryModel>,
    alias_routes: &HashMap<String, String>,
) -> HashMap<String, Vec<String>> {
    if declared_categories.is_empty() {
        return HashMap::new();
    }
    let mut carried = HashMap::new();
    for id in model_ids {
        let Some(model) = catalogue_model(id, registry_metadata, alias_routes) else {
            continue;
        };
        let names = categories_for(declared_categories, model);
        if !names.is_empty() {
            carried.insert(id.clone(), names);
        }
    }
    carried
}
