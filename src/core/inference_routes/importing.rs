//! Merging aliases an operator reviewed elsewhere into the registry: what one
//! reviewed alias carries, what the merge answers per alias, and the single
//! atomic rewrite that persists the accepted ones.

use std::collections::HashMap;
use std::path::Path;

use serde_json::Value;

use super::document::{
    ensure_parent, route_file_exists, snapshot, validate_document, write_registry,
    ROUTE_WRITE_LOCK, SCHEMA_VERSION,
};

#[derive(Debug, Clone)]
pub struct RouteImport {
    pub alias: String,
    pub primary: String,
    pub deployments: Vec<Value>,
    pub expected_primary: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouteImportDisposition {
    Imported,
    Unchanged,
    Conflicting,
}

#[derive(Debug, Clone)]
pub struct RouteImportResult {
    pub alias: String,
    pub disposition: RouteImportDisposition,
    pub detail: String,
}

/// Merge selected aliases from a previously validated import in one atomic
/// destination rewrite. Existing aliases and deployments win by default.
///
/// A deployment-name conflict is never replaced: doing so could redirect an
/// existing alias that was not part of the user's selection, without the user
/// ever seeing that alias in the review.
pub fn import_routes(
    path: &Path,
    imports: &[RouteImport],
    replace_alias_conflicts: bool,
) -> Result<(Value, Vec<RouteImportResult>), String> {
    let _guard = ROUTE_WRITE_LOCK
        .lock()
        .map_err(|_| "inference route write lock is poisoned".to_string())?;
    let mut value = if route_file_exists(path)? {
        snapshot(path)?
    } else {
        serde_json::json!({
            "schema_version": SCHEMA_VERSION,
            "deployments": [],
            "routes": {},
        })
    };
    validate_document(&value)?;
    let document = value
        .as_object_mut()
        .ok_or_else(|| "inference routes must be a JSON object".to_string())?;
    let existing_routes = document
        .entry("routes")
        .or_insert_with(|| Value::Object(Default::default()))
        .as_object()
        .ok_or_else(|| "inference routes.routes must be an object".to_string())?
        .clone();
    let existing_deployments = document
        .entry("deployments")
        .or_insert_with(|| Value::Array(Vec::new()))
        .as_array()
        .ok_or_else(|| "inference routes.deployments must be an array".to_string())?
        .clone();

    let mut deployment_by_name = HashMap::new();
    for deployment in existing_deployments {
        let name = deployment
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| "inference route deployment has no name".to_string())?;
        deployment_by_name.insert(name.to_string(), deployment);
    }

    let mut results = Vec::with_capacity(imports.len());
    let mut accepted = Vec::new();
    for route in imports {
        let imported_primary = Value::String(route.primary.clone());
        let current_primary = existing_routes.get(&route.alias);
        let expected_primary = route
            .expected_primary
            .as_ref()
            .map(|primary| Value::String(primary.clone()));
        if current_primary != expected_primary.as_ref() {
            results.push(RouteImportResult {
                alias: route.alias.clone(),
                disposition: RouteImportDisposition::Conflicting,
                detail: "the destination changed after the adoption review".to_string(),
            });
            continue;
        }
        if current_primary == Some(&imported_primary) {
            results.push(RouteImportResult {
                alias: route.alias.clone(),
                disposition: RouteImportDisposition::Unchanged,
                detail: "the destination already has this route".to_string(),
            });
            continue;
        }
        if current_primary.is_some() && !replace_alias_conflicts {
            results.push(RouteImportResult {
                alias: route.alias.clone(),
                disposition: RouteImportDisposition::Conflicting,
                detail: "the destination keeps its existing alias".to_string(),
            });
            continue;
        }
        let deployment_conflict = route.deployments.iter().find_map(|deployment| {
            let name = deployment.get("name").and_then(Value::as_str)?;
            deployment_by_name
                .get(name)
                .filter(|existing| *existing != deployment)
                .map(|_| name.to_string())
        });
        if let Some(name) = deployment_conflict {
            results.push(RouteImportResult {
                alias: route.alias.clone(),
                disposition: RouteImportDisposition::Conflicting,
                detail: format!(
                    "deployment '{name}' already exists with a different endpoint or adapter"
                ),
            });
            continue;
        }
        for deployment in &route.deployments {
            let name = deployment
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(|| "inference route deployment has no name".to_string())?;
            deployment_by_name
                .entry(name.to_string())
                .or_insert_with(|| deployment.clone());
        }
        accepted.push(route);
    }

    if !accepted.is_empty() {
        for route in accepted {
            document
                .get_mut("routes")
                .and_then(Value::as_object_mut)
                .ok_or_else(|| "inference routes.routes must be an object".to_string())?
                .insert(route.alias.clone(), Value::String(route.primary.clone()));
            results.push(RouteImportResult {
                alias: route.alias.clone(),
                disposition: RouteImportDisposition::Imported,
                detail: "the selected route was persisted".to_string(),
            });
        }
        let mut deployments = deployment_by_name.into_values().collect::<Vec<_>>();
        deployments.sort_by(|left, right| {
            left.get("name")
                .and_then(Value::as_str)
                .cmp(&right.get("name").and_then(Value::as_str))
        });
        document.insert("deployments".to_string(), Value::Array(deployments));
        validate_document(&value)?;
        ensure_parent(path)?;
        write_registry(path, &value)?;
    }
    results.sort_by(|left, right| left.alias.cmp(&right.alias));
    Ok((value, results))
}
