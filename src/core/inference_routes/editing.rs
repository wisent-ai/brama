//! Operator edits to a registry that already exists: setting one alias,
//! removing one alias, and moving a file written under an older document shape
//! onto the current one.
//!
//! All three take the registry write lock and hand the finished document to
//! [`write_registry`], which is the single place that stages, validates and
//! renames. None of them assembles its own temporary file.

use std::path::Path;

use serde_json::Value;

use super::document::{
    read_body, snapshot, validate_document, write_registry, ROUTE_WRITE_LOCK, SCHEMA_VERSION,
};

pub fn update_route(path: &Path, alias: &str, primary: &str) -> Result<Value, String> {
    let _guard = ROUTE_WRITE_LOCK
        .lock()
        .map_err(|_| "inference route write lock is poisoned".to_string())?;
    let mut value = snapshot(path)?;
    let document = value
        .as_object_mut()
        .ok_or_else(|| "inference routes must be a JSON object".to_string())?;
    let routes = document
        .entry("routes")
        .or_insert_with(|| Value::Object(Default::default()))
        .as_object_mut()
        .ok_or_else(|| "inference routes.routes must be an object".to_string())?;
    routes.insert(alias.to_string(), Value::String(primary.to_string()));
    write_registry(path, &value)?;
    Ok(value)
}

pub fn delete_route(path: &Path, alias: &str) -> Result<Value, String> {
    let _guard = ROUTE_WRITE_LOCK
        .lock()
        .map_err(|_| "inference route write lock is poisoned".to_string())?;
    let mut value = snapshot(path)?;
    let document = value
        .as_object_mut()
        .ok_or_else(|| "inference routes must be a JSON object".to_string())?;
    let routes = document
        .get_mut("routes")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| "inference routes.routes must be an object".to_string())?;
    if routes.remove(alias).is_none() {
        return Err("route alias not found".to_string());
    }
    write_registry(path, &value)?;
    Ok(value)
}

/// Rewrite an operator's registry file so it holds exactly the members the
/// current document defines: `schema_version`, `deployments` and `routes`.
///
/// This exists so an operator whose file was written under an earlier shape
/// moves onto the current one by running Brama, never by hand-editing JSON.
/// The document is read as raw JSON rather than through the typed loader,
/// because the typed loader is precisely what a retired member no longer
/// passes; the same refusals still apply first, so a symlink, a file owned by
/// somebody else, or one readable by group or other is rejected here exactly as
/// it is on every other read. Members that are absent are written with the
/// empty value the loader would have assumed, so running it twice produces the
/// same document as running it once.
pub fn migrate(path: &Path) -> Result<Value, String> {
    let _guard = ROUTE_WRITE_LOCK
        .lock()
        .map_err(|_| "inference route write lock is poisoned".to_string())?;
    let body = read_body(path)?;
    let value: Value =
        serde_json::from_str(&body).map_err(|error| format!("invalid inference routes: {error}"))?;
    let document = value
        .as_object()
        .ok_or_else(|| "inference routes must be a JSON object".to_string())?;
    let mut migrated = serde_json::Map::new();
    migrated.insert(
        "schema_version".to_string(),
        document
            .get("schema_version")
            .cloned()
            .unwrap_or_else(|| Value::from(SCHEMA_VERSION)),
    );
    migrated.insert(
        "deployments".to_string(),
        document
            .get("deployments")
            .cloned()
            .unwrap_or_else(|| Value::Array(Vec::new())),
    );
    migrated.insert(
        "routes".to_string(),
        document
            .get("routes")
            .cloned()
            .unwrap_or_else(|| Value::Object(Default::default())),
    );
    let migrated = Value::Object(migrated);
    validate_document(&migrated)?;
    write_registry(path, &migrated)?;
    Ok(migrated)
}
